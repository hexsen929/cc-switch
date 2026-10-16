#!/usr/bin/env python3
"""Load exported application TOML with the pinned official Codex CLI, without a turn.

Only initialize and config/read are sent. No login, thread/start, turn/start,
model discovery, exec, MCP connect, or tool invocation is part of this contract.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import queue
import re
import shutil
import subprocess
import tempfile
import threading
import time
import tomllib

VERSION = "0.162.1"
UNKNOWN_FIELD = "cc_switch_contract_unknown_field"
WARNING = re.compile(r"unknown (?:config(?:uration)? )?(?:field|key)|unrecognized (?:field|key)", re.I)


class ContractFailure(RuntimeError):
    pass


def sanitized_failure(error: Exception) -> str:
    """For explicitly supplied private configs, never echo values, paths or raw errors."""
    text = str(error)
    fields = re.findall(r"unknown configuration field `([A-Za-z0-9_.-]+)`", text)
    if fields:
        return "unknown configuration field: " + ", ".join(dict.fromkeys(fields))
    if "strict-config" in text and "unknown-field" in text:
        return "strict-config negative control failed"
    if "no result" in text:
        return "configuration could not be loaded (raw diagnostics withheld)"
    if "other than the isolated fixture" in text or "user layer" in text:
        return "configuration layer isolation check failed"
    return "configuration validation failed (raw diagnostics withheld)"


def fixture_environment(home: Path) -> dict[str, str]:
    # Inherit neither user auth nor a real CODEX_HOME, proxy, telemetry or plugin config.
    return {
        "PATH": os.environ.get("PATH", ""),
        "HOME": str(home),
        "CODEX_HOME": str(home / "codex"),
        "XDG_CONFIG_HOME": str(home / "xdg"),
        "OPENAI_API_KEY": "sk-contract-fixture-not-a-real-key",
        "HTTP_PROXY": "http://127.0.0.1:9",
        "HTTPS_PROXY": "http://127.0.0.1:9",
        "ALL_PROXY": "http://127.0.0.1:9",
        "NO_PROXY": "127.0.0.1,localhost",
        "RUST_LOG": "error",
        "DO_NOT_TRACK": "1",
        "TERM": "dumb",
    }


def send(process: subprocess.Popen[str], message: dict) -> None:
    assert process.stdin is not None
    process.stdin.write(json.dumps(message) + "\n")
    process.stdin.flush()


def load_config(binary: str, text: str, reject: bool = False) -> dict | None:
    with tempfile.TemporaryDirectory(prefix="cc-switch-codex-contract-") as temporary:
        home = Path(temporary)
        config_home = home / "codex"
        config_home.mkdir()
        config_path = config_home / "config.toml"
        config_path.write_text(text, encoding="utf-8")
        (config_home / "auth.json").write_text(
            json.dumps({"OPENAI_API_KEY": "sk-contract-fixture-not-a-real-key"}), encoding="utf-8"
        )
        process = subprocess.Popen(
            [binary, "--strict-config", "app-server", "--listen", "stdio://"],
            cwd=home,
            env=fixture_environment(home),
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
        )
        events: queue.Queue[tuple[str, str]] = queue.Queue()

        def read_stream(stream, channel: str) -> None:
            for line in stream:
                events.put((channel, line))
            events.put((channel, ""))

        assert process.stdout is not None and process.stderr is not None
        readers = [
            threading.Thread(target=read_stream, args=(process.stdout, "stdout"), daemon=True),
            threading.Thread(target=read_stream, args=(process.stderr, "stderr"), daemon=True),
        ]
        for reader in readers:
            reader.start()
        stderr: list[str] = []
        result = None
        closed: set[str] = set()
        try:
            send(process, {
                "id": 1,
                "method": "initialize",
                "params": {
                    "clientInfo": {"name": "cc-switch-config-contract", "version": "1.0.0"},
                    "capabilities": {"experimentalApi": True},
                },
            })
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline and len(closed) < 2:
                try:
                    channel, line = events.get(timeout=min(0.25, max(deadline - time.monotonic(), 0.01)))
                except queue.Empty:
                    continue
                if not line:
                    closed.add(channel)
                    continue
                if channel == "stderr":
                    stderr.append(line)
                    continue
                try:
                    message = json.loads(line)
                except json.JSONDecodeError as error:
                    raise ContractFailure(f"app-server emitted non-JSON stdout: {line[:300]}") from error
                if message.get("id") in (1, 2) and "error" in message:
                    raise ContractFailure(f"app-server rejected configuration: {message['error']}")
                if "warning" in message.get("method", "").lower():
                    raise ContractFailure(f"app-server configuration warning: {message}")
                if message.get("id") == 1:
                    send(process, {"method": "initialized"})
                    send(process, {"id": 2, "method": "config/read", "params": {"includeLayers": True}})
                if message.get("id") == 2:
                    result = message.get("result")
                    break
            if reject:
                if result is not None:
                    raise ContractFailure("strict-config accepted the unknown-field negative control")
                exit_code = process.wait(timeout=5)
                # The reader thread may still be transferring the final stderr line.
                for reader in readers:
                    reader.join(timeout=1)
                while not events.empty():
                    channel, line = events.get_nowait()
                    if channel == "stderr":
                        stderr.append(line)
                if exit_code == 0 or UNKNOWN_FIELD not in "".join(stderr) or not WARNING.search("".join(stderr)):
                    raise ContractFailure(f"negative control failed for the wrong reason: exit={exit_code}, stderr={''.join(stderr)[-2000:]}")
                return None
            if result is None:
                raise ContractFailure(f"config/read produced no result: {''.join(stderr)[-2000:]}")
            if WARNING.search("".join(stderr)):
                raise ContractFailure(f"unknown configuration warning: {''.join(stderr)[-2000:]}")
            parsed = tomllib.loads(text)
            layers = result.get("layers") or []
            user_layers = [layer for layer in layers if layer.get("name", {}).get("type") == "user" and Path(layer["name"]["file"]).resolve() == config_path.resolve()]
            if not user_layers or user_layers[-1].get("config") != parsed:
                raise ContractFailure("config/read did not load exactly the fixture user layer")
            effective = result.get("config")
            if not isinstance(effective, dict):
                raise ContractFailure("config/read omitted effective configuration")
            for key in ("model", "model_provider", "model_reasoning_effort", "approval_policy", "sandbox_mode"):
                if key in parsed and effective.get(key) != parsed[key]:
                    raise ContractFailure(f"effective configuration ignored {key}: expected {parsed[key]!r}, got {effective.get(key)!r}")
            for provider_id, expected in parsed.get("model_providers", {}).items():
                actual = effective.get("model_providers", {}).get(provider_id, {})
                for key, value in expected.items():
                    if actual.get(key) != value:
                        raise ContractFailure(f"effective provider ignored model_providers.{provider_id}.{key}")
            for key in ("model", "model_provider"):
                if key in parsed:
                    origin = result.get("origins", {}).get(key, {}).get("name", {})
                    if origin.get("type") != "user" or Path(origin.get("file", "/missing")).resolve() != config_path.resolve():
                        raise ContractFailure(f"{key} came from a layer other than the isolated fixture")
            return result
        finally:
            if process.poll() is None:
                process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
            for reader in readers:
                reader.join(timeout=1)
            for stream in (process.stdin, process.stdout, process.stderr):
                if stream:
                    stream.close()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--codex", default="codex", help="Official CLI executable (must be pinned version)")
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--fixtures", type=Path)
    source.add_argument("--config-file", type=Path, help="Copy one explicitly supplied config into isolated temporary homes; never reads its auth.json")
    parser.add_argument("--report", type=Path)
    parser.add_argument("--allow-version", action="store_true", help="Single-file diagnostics only: allow an explicitly chosen installed CLI version")
    args = parser.parse_args()
    binary = shutil.which(args.codex)
    if not binary:
        raise ContractFailure(f"Codex CLI executable not found: {args.codex}")
    version = subprocess.check_output([binary, "--version"], text=True).strip()
    if args.allow_version and not args.config_file:
        raise ContractFailure("--allow-version is restricted to single-file diagnostics")
    if not args.allow_version and version != f"codex-cli {VERSION}":
        raise ContractFailure(f"expected codex-cli {VERSION}, got {version}")
    if args.config_file:
        try:
            load_config(binary, args.config_file.read_text(encoding="utf-8"))
        except Exception as error:
            print(sanitized_failure(error))
            raise SystemExit(1) from None
        print("PASS: isolated strict configuration load and config/read; no model request sent.")
        return
    if not args.report:
        raise ContractFailure("--report is required for the CI fixture suite")
    for text in (
        f'{UNKNOWN_FIELD} = true\n',
        f'[model_providers.custom]\nname = "Fixture"\nwire_api = "responses"\n{UNKNOWN_FIELD} = true\n',
        f'[features]\n{UNKNOWN_FIELD} = true\n',
    ):
        load_config(binary, text, reject=True)
    manifest = json.loads((args.fixtures / "manifest.json").read_text(encoding="utf-8"))
    if len(manifest) < 20:
        raise ContractFailure("fixture export is unexpectedly incomplete")
    report = {"version": VERSION, "methods": ["initialize", "config/read"], "negativeControls": 3, "fixtures": []}
    failures = []
    for fixture in manifest:
        try:
            text = (args.fixtures / f"{fixture['id']}.toml").read_text(encoding="utf-8")
            load_config(binary, text)
            report["fixtures"].append({**fixture, "status": "passed"})
            print(f"PASS {fixture['name']}", flush=True)
        except Exception as error:
            failures.append(f"{fixture['name']}: {error}")
            report["fixtures"].append({**fixture, "status": "failed", "error": str(error)})
            print(f"FAIL {fixture['name']}: {error}", flush=True)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    if failures:
        raise ContractFailure("\n".join(failures))
    print(f"Validated {len(manifest)} actual configurations with Codex {VERSION}; 3 strict unknown-field controls rejected.")


if __name__ == "__main__":
    main()
