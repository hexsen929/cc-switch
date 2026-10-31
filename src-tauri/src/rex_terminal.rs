//! Rex owns its sessions on a shared server. Select the requested local app
//! explicitly: both installed variants register the same `rex://` scheme.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde_json::Value;

const CLI_RELATIVE_PATH: &str = "Contents/Helpers/Rex Server.rex/Contents/MacOS/rex";

/// A submission can reach the server even if the CLI loses its response. Keep
/// private launcher files alive in that case, and never launch a fallback.
#[derive(Debug)]
pub(crate) struct LaunchError {
    message: String,
    pub(crate) may_have_started: bool,
}

impl LaunchError {
    fn after_submission(message: String) -> Self {
        Self {
            message,
            may_have_started: true,
        }
    }
}

impl From<String> for LaunchError {
    fn from(message: String) -> Self {
        Self {
            message,
            may_have_started: false,
        }
    }
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for LaunchError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Variant {
    Original,
    Chinese,
}

impl Variant {
    fn from_bundle_id(bundle_id: &str) -> Result<Self, String> {
        match bundle_id.trim() {
            "com.superlogical.rex" => Ok(Self::Original),
            "com.superlogical.rex.zh-hans" => Ok(Self::Chinese),
            _ => Err(format!("未知的 Rex 应用标识：{}", bundle_id.trim())),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Original => "Rex Beta",
            Self::Chinese => "Rex Beta 中文",
        }
    }
}

/// Both settings select the same automatically detected app. Keep the old
/// `rex-zh` setting working, and prefer the Chinese install while both exist.
fn app_candidates(target: &str, home: &Path) -> Result<[PathBuf; 4], String> {
    if !matches!(target, "rex" | "rex-zh") {
        return Err(format!("未知的 Rex 终端选项：{target}"));
    }
    Ok([
        Path::new("/Applications").join("Rex Beta 中文.app"),
        home.join("Applications").join("Rex Beta 中文.app"),
        Path::new("/Applications").join("Rex Beta.app"),
        home.join("Applications").join("Rex Beta.app"),
    ])
}

/// Explicitly select the local server, regardless of inherited Rex context or
/// the user's remote home-server selection. URL-encode spaces and Unicode.
fn local_server_endpoint(home: &Path) -> Result<String, String> {
    let home = home
        .to_str()
        .ok_or_else(|| "Rex 本地服务器路径包含非 UTF-8 字符".to_string())?;
    let mut endpoint = url::Url::parse("unix:///").map_err(|e| e.to_string())?;
    endpoint.set_path(&format!(
        "{}/Library/Application Support/rex/server.sock",
        home.trim_end_matches('/')
    ));
    Ok(endpoint.to_string())
}

fn cli_args(endpoint: &str) -> Vec<OsString> {
    ["--server", endpoint, "--autostart=false", "--timeout", "2s"]
        .into_iter()
        .map(OsString::from)
        .collect()
}

fn session_args(command: &[OsString], cwd: Option<&str>, direct: bool) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "new",
        "CC-Switch",
        "--window",
        "Terminal",
        "--keep-open",
        "--json",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    if direct {
        args.push("--shell=none".into());
    }
    if let Some(cwd) = cwd.filter(|value| !value.trim().is_empty()) {
        args.extend([OsString::from("-c"), OsString::from(cwd)]);
    }
    args.push("--".into());
    args.extend_from_slice(command);
    args
}

fn selected_client(clients: &Value, variant: Variant) -> Result<Option<String>, String> {
    let clients = clients
        .get("clients")
        .and_then(Value::as_array)
        .ok_or_else(|| "Rex CLI 未返回有效的客户端列表".to_string())?;
    let mut matching = std::collections::BTreeSet::new();
    for client in clients {
        if client["connection_state"] != "connected"
            || client["info"]["kind"] != "app"
            || client["info"]["name"] != variant.name()
            || client["principal"]["kind"] != "peer"
            || client["principal"]["transport"] != "unix"
        {
            continue;
        }
        if let Some(id) = client["client_id"]
            .as_str()
            .filter(|id| id.starts_with("client:") && id.len() > "client:".len())
        {
            matching.insert(id.to_string());
        }
    }
    if matching.len() > 1 {
        return Err(format!(
            "检测到多个 {} 本地客户端，请只保留一个再试",
            variant.name()
        ));
    }
    Ok(matching.into_iter().next())
}

fn created_session(stdout: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(stdout).ok()?;
    value["session_id"]
        .as_str()
        .filter(|id| id.starts_with("session:") && id.len() > "session:".len())
        .map(str::to_string)
}

#[cfg(target_os = "macos")]
fn rex_command(cli: &Path, endpoint: &str) -> std::process::Command {
    let mut command = std::process::Command::new(cli);
    command.args(cli_args(endpoint));
    for name in ["REX_SERVER", "REX_SESSION", "REX_BLOCK", "REX_CLIENT"] {
        command.env_remove(name);
    }
    command
}

#[cfg(target_os = "macos")]
fn output_error(output: &std::process::Output) -> String {
    let detail: String = String::from_utf8_lossy(&output.stderr)
        .trim()
        .chars()
        .take(1000)
        .collect();
    if detail.is_empty() {
        format!("exit code: {:?}", output.status.code())
    } else {
        detail
    }
}

#[cfg(target_os = "macos")]
fn app_variant(app: &Path) -> Result<Variant, String> {
    // A Finder rename changes only the directory, not Rex's bundle identity or
    // the name it reports in `client ls`. Do not infer either from the path.
    let output = std::process::Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Print :CFBundleIdentifier"])
        .arg(app.join("Contents/Info.plist"))
        .output()
        .map_err(|e| format!("读取 Rex 应用标识失败：{e}"))?;
    if !output.status.success() {
        return Err(format!("读取 Rex 应用标识失败：{}", output_error(&output)));
    }
    let bundle_id =
        std::str::from_utf8(&output.stdout).map_err(|e| format!("Rex 应用标识格式错误：{e}"))?;
    Variant::from_bundle_id(bundle_id)
}

#[cfg(target_os = "macos")]
fn wait_for_client(cli: &Path, endpoint: &str, variant: Variant) -> Result<String, String> {
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + Duration::from_secs(8);
    let mut detail = String::new();
    loop {
        match rex_command(cli, endpoint)
            .args(["client", "ls", "--json"])
            .output()
        {
            Ok(output) if output.status.success() => {
                let clients: Value = serde_json::from_slice(&output.stdout)
                    .map_err(|e| format!("Rex 客户端列表解析失败：{e}"))?;
                if let Some(client) = selected_client(&clients, variant)? {
                    return Ok(client);
                }
            }
            Ok(output) => detail = output_error(&output),
            Err(error) => return Err(format!("启动 Rex CLI 失败：{error}")),
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "{} 尚未连接本地 Rex 服务器，请确认应用启动正常后重试。{detail}",
                variant.name()
            ));
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

#[cfg(target_os = "macos")]
fn launch(
    target: &str,
    command: &[OsString],
    cwd: Option<&str>,
    direct: bool,
) -> Result<(), LaunchError> {
    use std::os::unix::fs::PermissionsExt;

    let home = dirs::home_dir().ok_or_else(|| "未找到 Rex 用户目录".to_string())?;
    let (app, cli) = app_candidates(target, &home)?
        .into_iter()
        .find_map(|app| {
            let cli = app.join(CLI_RELATIVE_PATH);
            let executable = cli.metadata().is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            });
            executable.then_some((app, cli))
        })
        .ok_or_else(|| {
            "未找到 Rex 的内置 CLI，请将应用安装到 /Applications 或 ~/Applications".to_string()
        })?;
    let variant = app_variant(&app)?;
    let endpoint = local_server_endpoint(&home)?;
    let opened = std::process::Command::new("/usr/bin/open")
        .arg("-a")
        .arg(&app)
        .output()
        .map_err(|e| format!("启动 {} 失败：{e}", variant.name()))?;
    if !opened.status.success() {
        return Err(format!("启动 {} 失败：{}", variant.name(), output_error(&opened)).into());
    }
    let client = wait_for_client(&cli, &endpoint, variant)?;

    // Submit exactly once. A failed/ambiguous response must not trigger another
    // `new`, or a fallback terminal: it may already be executing the command.
    let child = rex_command(&cli, &endpoint)
        .args(session_args(command, cwd, direct))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Rex 新建会话失败：{e}"))?;
    let created = child.wait_with_output().map_err(|e| {
        LaunchError::after_submission(format!(
            "Rex 新建会话响应未确认，请先检查 Rex 后再重试：{e}"
        ))
    })?;
    if !created.status.success() {
        return Err(LaunchError::after_submission(format!(
            "Rex 新建会话未确认，请先检查 Rex 后再重试：{}",
            output_error(&created)
        )));
    }

    // Once Rex acknowledged execution, presentation errors are not launch
    // failures. Returning Err here would invite callers to run it a second time
    // or remove private launcher files that the new terminal still needs.
    let Some(session) = created_session(&created.stdout) else {
        log::warn!("Rex 已确认新建会话，但未返回可识别的 session_id；不重复执行命令");
        return Ok(());
    };
    match rex_command(&cli, &endpoint)
        .args(["-C", &client, "do", "session.select"])
        .arg(format!("session_id={session}"))
        .output()
    {
        Ok(output) if output.status.success() => {}
        Ok(output) => log::warn!(
            "Rex 会话 {session} 已启动，但在 {} 中显示失败：{}。如提示 Remote Control，请在 Rex Settings → Rex Server 中启用它；也可在 Rex 侧栏选择 CC-Switch 会话。不会重复执行命令。",
            variant.name(),
            output_error(&output)
        ),
        Err(error) => log::warn!("Rex 会话 {session} 已启动，但显示失败：{error}"),
    }
    Ok(())
}

/// Execute the existing private launcher without exposing its contents in Rex
/// arguments. The launcher retains ownership of its env/prompt snapshots.
#[cfg(target_os = "macos")]
pub(crate) fn launch_script(target: &str, script_file: &Path) -> Result<(), LaunchError> {
    if !script_file.is_file() {
        return Err("Rex 启动脚本不存在".to_string().into());
    }
    launch(
        target,
        &[
            OsString::from("/bin/sh"),
            script_file.as_os_str().to_owned(),
        ],
        None,
        true,
    )
}

/// Native session validation happens at the caller. Keep cwd as one argument,
/// and let Rex obtain the login environment before starting the resume command.
#[cfg(target_os = "macos")]
pub(crate) fn launch_command(target: &str, command: &str, cwd: Option<&str>) -> Result<(), String> {
    if command.trim().is_empty() {
        return Err("Rex 恢复命令为空".to_string());
    }
    launch(
        target,
        &[
            OsString::from("/bin/sh"),
            OsString::from("-c"),
            OsString::from(command),
        ],
        cwd,
        false,
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn automatic_selection_prefers_chinese_app_and_keeps_legacy_alias() {
        let home = Path::new("/Users/test user");
        let candidates = app_candidates("rex", home).unwrap();
        assert_eq!(
            candidates,
            [
                PathBuf::from("/Applications/Rex Beta 中文.app"),
                home.join("Applications/Rex Beta 中文.app"),
                PathBuf::from("/Applications/Rex Beta.app"),
                home.join("Applications/Rex Beta.app"),
            ]
        );
        assert_eq!(candidates, app_candidates("rex-zh", home).unwrap());
        assert_eq!(
            CLI_RELATIVE_PATH,
            "Contents/Helpers/Rex Server.rex/Contents/MacOS/rex"
        );
        assert!(app_candidates("rex://anything", home).is_err());
    }

    #[test]
    fn renamed_chinese_app_selects_client_by_bundle_identity_not_directory_name() {
        let renamed = PathBuf::from("/Applications/Rex Beta.app");
        assert!(app_candidates("rex", Path::new("/Users/test"))
            .unwrap()
            .contains(&renamed));
        let variant = Variant::from_bundle_id("com.superlogical.rex.zh-hans\n").unwrap();
        assert_eq!(variant, Variant::Chinese);
        let clients = json!({"clients": [
            client("client:original", "Rex Beta"),
            client("client:renamed-chinese", "Rex Beta 中文")
        ]});
        assert_eq!(
            selected_client(&clients, variant).unwrap().as_deref(),
            Some("client:renamed-chinese")
        );
        assert_eq!(
            Variant::from_bundle_id("com.superlogical.rex").unwrap(),
            Variant::Original
        );
        assert!(Variant::from_bundle_id("com.example.other").is_err());
    }

    #[test]
    fn local_endpoint_is_explicit_and_percent_encodes_home() {
        let endpoint = local_server_endpoint(Path::new("/Users/测试 name/#dir")).unwrap();
        assert_eq!(
            endpoint,
            "unix:///Users/%E6%B5%8B%E8%AF%95%20name/%23dir/Library/Application%20Support/rex/server.sock"
        );
        assert_eq!(
            cli_args(&endpoint),
            [
                "--server",
                &endpoint,
                "--autostart=false",
                "--timeout",
                "2s"
            ]
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>()
        );
    }

    #[test]
    fn private_launcher_paths_are_individual_arguments_without_shell_expansion() {
        let script = "/tmp/中文 dir/it's $(touch unwanted).sh";
        let args = session_args(&["/bin/sh".into(), script.into()], None, true);
        assert_eq!(
            args,
            [
                "new",
                "CC-Switch",
                "--window",
                "Terminal",
                "--keep-open",
                "--json",
                "--shell=none",
                "--",
                "/bin/sh",
                script
            ]
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>()
        );
    }

    #[test]
    fn resume_keeps_command_and_working_directory_separate() {
        let command = "claude --resume 'session-id'";
        let cwd = "/tmp/中文 dir/it's $(touch unwanted)";
        let args = session_args(
            &["/bin/sh".into(), "-c".into(), command.into()],
            Some(cwd),
            false,
        );
        assert!(!args.contains(&OsString::from("--shell=none")));
        let separator = args.iter().position(|arg| arg == "--").unwrap();
        assert_eq!(
            &args[separator - 2..separator],
            &[OsString::from("-c"), cwd.into()]
        );
        assert_eq!(
            &args[separator + 1..],
            &["/bin/sh", "-c", command].map(OsString::from)
        );
    }

    fn client(id: &str, name: &str) -> Value {
        json!({
            "client_id": id,
            "connection_state": "connected",
            "info": {"kind": "app", "name": name},
            "principal": {"kind": "peer", "transport": "unix"}
        })
    }

    #[test]
    fn selection_ignores_other_variant_cli_disconnected_and_remote_clients() {
        let original = client("client:original", "Rex Beta");
        let chinese = client("client:chinese", "Rex Beta 中文");
        let mut cli = client("client:cli", "Rex Beta 中文");
        cli["info"]["kind"] = json!("cli");
        let mut remote = client("client:remote", "Rex Beta 中文");
        remote["principal"]["transport"] = json!("tailscale");
        let mut disconnected = client("client:gone", "Rex Beta 中文");
        disconnected["connection_state"] = json!("disconnected");
        let list = json!({"clients": [original, chinese, cli, remote, disconnected]});
        assert_eq!(
            selected_client(&list, Variant::Original)
                .unwrap()
                .as_deref(),
            Some("client:original")
        );
        assert_eq!(
            selected_client(&list, Variant::Chinese).unwrap().as_deref(),
            Some("client:chinese")
        );
        assert_eq!(
            selected_client(&json!({"clients": []}), Variant::Chinese).unwrap(),
            None
        );
    }

    #[test]
    fn ambiguous_clients_are_rejected_before_any_session_is_created() {
        let list =
            json!({"clients": [client("client:a", "Rex Beta"), client("client:b", "Rex Beta")]});
        assert!(selected_client(&list, Variant::Original).is_err());
        assert!(selected_client(&json!({}), Variant::Original).is_err());
    }

    #[test]
    fn session_response_parsing_never_requests_a_second_creation() {
        assert_eq!(
            created_session(br#"{"session_id":"session:created"}"#).as_deref(),
            Some("session:created")
        );
        assert_eq!(created_session(b"malformed response"), None);
        assert_eq!(created_session(br#"{"session_id":""}"#), None);
        assert_eq!(
            created_session(br#"{"session_id":"client:wrong-kind"}"#),
            None
        );
    }

    #[test]
    fn launch_errors_distinguish_safe_cleanup_from_uncertain_submission() {
        let missing_app = LaunchError::from("app missing".to_string());
        assert!(!missing_app.may_have_started);
        assert_eq!(missing_app.to_string(), "app missing");

        let lost_response = LaunchError::after_submission("response lost".to_string());
        assert!(lost_response.may_have_started);
        assert_eq!(lost_response.to_string(), "response lost");
    }
}
