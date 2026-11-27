//! Dedicated choice protocols. The upstream origin and credential come from
//! the selected card, never from a classification result or a client header.
use super::{Protocol, SmartConfig, Target, Turn};
use crate::{
    provider::Provider,
    proxy::{
        handler_context::RequestContext,
        http_client,
        hyper_client::ProxyResponse,
        pii_redaction, privacy_filter,
        providers::{get_adapter, AuthStrategy},
        server::ProxyState,
        upstream_guard,
    },
};
use serde_json::{json, Value};
use url::Url;

fn endpoint(raw: &str, protocol: Protocol) -> Result<Url, ()> {
    let mut url = Url::parse(raw).map_err(|_| ())?;
    if !url.has_host()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !(url.scheme() == "https" || url.scheme() == "http" && http_client::is_loopback_url(raw))
    {
        return Err(());
    }
    let base = url.path().trim_end_matches('/');
    let leaf = match protocol {
        Protocol::OpenaiDecisions | Protocol::GoogleDecisions => "decisions",
        Protocol::SystemOne => "systemone",
        Protocol::Chat => return Err(()),
    };
    if base.rsplit('/').next() == Some(leaf) {
        return Ok(url);
    }
    if [
        "chat/completions",
        "messages",
        "responses",
        "generateContent",
    ]
    .iter()
    .any(|endpoint| base.ends_with(endpoint))
    {
        return Err(());
    }
    let base = if base.is_empty() {
        if protocol == Protocol::GoogleDecisions {
            "/v1beta"
        } else {
            "/v1"
        }
    } else {
        base
    };
    let path = format!("{base}/{leaf}");
    url.set_path(&path);
    Ok(url)
}

/// Encode the normalized excerpts after filtering in an ordinary message
/// envelope, so both existing privacy filters cover *all* native protocols.
fn body(
    cfg: &SmartConfig,
    target: &Target,
    turn: &Turn,
    previous: Option<usize>,
    mut clean: impl FnMut(&str) -> String,
) -> Value {
    let mut state = json!({"message":clean(&turn.text)});
    if !turn.previous.is_empty() {
        state["previous_user_message"] = json!(clean(&turn.previous));
    }
    if !turn.reply.is_empty() {
        state["previous_assistant_reply"] = json!(clean(&turn.reply));
    }
    if let Some(p) = previous {
        state["previous_message_kind"] = json!((p + 1).to_string());
    }
    let question = format!(
        "{}\nClassify the message, using the preceding turn for a continuation. Use 0 if none fit.",
        clean(&cfg.prompt)
    );
    let choices: Vec<(String, String)> =
        std::iter::once(("0".into(), "None of these kinds".into()))
            .chain(
                cfg.categories
                    .iter()
                    .enumerate()
                    .map(|(i, c)| ((i + 1).to_string(), clean(&c.description))),
            )
            .collect();
    if target.protocol == Protocol::OpenaiDecisions {
        let choices: Vec<_> = choices
            .into_iter()
            .map(|(value, description)| json!({"value":value,"description":description}))
            .collect();
        json!({"model":target.model,"input":[{"type":"message","role":"user","content":[{"type":"input_json","data":state}]}],
            "questions":[{"type":"choice","name":"intent","instructions":question,"choices":choices}]})
    } else {
        let criteria: serde_json::Map<String, Value> = choices
            .into_iter()
            .map(|(id, description)| (id, Value::String(description)))
            .collect();
        let mut ask = json!({"model":target.model,"questions":{"intent":{"type":"choice","instructions":question,"criteria":criteria}}});
        if target.protocol == Protocol::SystemOne {
            ask["state"] = state;
        } else {
            ask["input"] = json!(state.to_string());
        }
        ask
    }
}

fn verdict(
    protocol: Protocol,
    response: &Value,
    count: usize,
    min: u8,
) -> Result<Option<usize>, ()> {
    if !response["error"].is_null() {
        return Err(());
    }
    let answer = if protocol == Protocol::OpenaiDecisions {
        let answers = response["answers"].as_array().ok_or(())?;
        if answers.len() != 1 || answers[0]["name"] != "intent" {
            return Err(());
        }
        &answers[0]
    } else {
        response.pointer("/answers/intent").ok_or(())?
    };
    if answer["type"].as_str().is_some_and(|ty| ty != "choice")
        || (protocol == Protocol::OpenaiDecisions && answer["type"] != "choice")
    {
        return Err(());
    }
    let choice = answer["choice"].as_str().ok_or(())?;
    let id = choice
        .parse::<usize>()
        .ok()
        .filter(|i| *i <= count && i.to_string() == choice)
        .ok_or(())?;
    if id == 0 {
        return Ok(None);
    }
    let v = answer["confidence"].as_f64().or_else(|| {
        let ps = &answer["probabilities"];
        if let Some(a) = ps.as_array() {
            a.iter()
                .find(|p| p["value"] == choice)
                .and_then(|p| p["probability"].as_f64())
        } else {
            ps.get(choice).and_then(Value::as_f64)
        }
    });
    match v {
        Some(s) if (0.0..=1.0).contains(&s) => Ok((s * 100.0 >= f64::from(min)).then_some(id - 1)),
        None if min == 0 => Ok(Some(id - 1)),
        _ => Err(()),
    }
}

pub(super) async fn ask(
    ctx: &RequestContext,
    state: &ProxyState,
    cfg: &SmartConfig,
    target: &Target,
    p: &Provider,
    turn: &Turn,
    previous: Option<usize>,
) -> Result<Option<usize>, ()> {
    let adapter = get_adapter(&ctx.app_type).ok_or(())?;
    let url = endpoint(
        &adapter.extract_base_url(p).map_err(|_| ())?,
        target.protocol,
    )?;
    let auth = adapter.extract_auth(p).ok_or(())?;
    if !matches!(
        auth.strategy,
        AuthStrategy::Bearer
            | AuthStrategy::ClaudeAuth
            | AuthStrategy::Anthropic
            | AuthStrategy::Google
    ) || auth.api_key.trim().is_empty()
        || auth.api_key == "PROXY_MANAGED"
    {
        return Err(());
    }
    let (addr, port) = {
        let conf = state.config.read().await;
        (conf.listen_address.clone(), conf.listen_port)
    };
    upstream_guard::reject_self_forwarding(url.as_str(), &addr, port)
        .await
        .map_err(|_| ())?;
    for (addr, port) in http_client::active_proxy_listeners() {
        upstream_guard::reject_self_forwarding(url.as_str(), &addr, port)
            .await
            .map_err(|_| ())?;
    }
    let q = body(cfg, target, turn, previous, |s| {
        let mut envelope = json!({"messages":[{"role":"user","content":s}]});
        if ctx.privacy_filter_config.enabled {
            privacy_filter::redact_request_body(
                &mut envelope,
                privacy_filter::Placeholders::for_language(&crate::tray::tray_language()),
                false,
            );
        }
        pii_redaction::apply_pii_redaction(&mut envelope, &ctx.pii_redaction);
        envelope["messages"][0]["content"]
            .as_str()
            .unwrap_or("")
            .to_owned()
    });
    let client = http_client::no_redirect_for_url(url.as_str()).map_err(|_| ())?;
    let azure = url
        .host_str()
        .is_some_and(|h| h.ends_with(".openai.azure.com"));
    let post = client.post(url).header(super::MARKER, "1");
    let post = match target.protocol {
        Protocol::GoogleDecisions => post.header("x-goog-api-key", auth.api_key.as_str()),
        Protocol::OpenaiDecisions if azure => post.header("api-key", auth.api_key.as_str()),
        _ => post.bearer_auth(&auth.api_key),
    };
    let res = post.json(&q).send().await.map_err(|_| ())?;
    if !res.status().is_success() {
        return Err(());
    }
    let bytes = ProxyResponse::Reqwest(res)
        .bytes_with_limit(128 * 1024)
        .await
        .map_err(|_| ())?;
    verdict(
        target.protocol,
        &serde_json::from_slice(&bytes).map_err(|_| ())?,
        cfg.categories.len(),
        cfg.min_confidence,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_preserve_card_origin_and_reject_secret_urls_and_plaintext() {
        for (base, kind, out) in [
            (
                "https://api.openai.com",
                Protocol::OpenaiDecisions,
                "https://api.openai.com/v1/decisions",
            ),
            (
                "https://judge.test/api/v1",
                Protocol::SystemOne,
                "https://judge.test/api/v1/systemone",
            ),
            (
                "https://judge.test/v1/systemone",
                Protocol::SystemOne,
                "https://judge.test/v1/systemone",
            ),
            (
                "https://generativelanguage.googleapis.com",
                Protocol::GoogleDecisions,
                "https://generativelanguage.googleapis.com/v1beta/decisions",
            ),
        ] {
            assert_eq!(endpoint(base, kind).unwrap().as_str(), out);
        }
        for raw in [
            "https://u:p@judge.test/v1",
            "http://judge.test/v1",
            "https://judge.test/v1?key=private",
            "https://judge.test/v1/chat/completions",
            "file:///tmp/model",
        ] {
            assert!(endpoint(raw, Protocol::OpenaiDecisions).is_err());
        }
    }

    #[test]
    fn refusals_unknown_choices_invalid_probabilities_and_low_confidence_are_distinct() {
        let decode = |answer: Value| {
            verdict(
                Protocol::OpenaiDecisions,
                &json!({"answers":[answer]}),
                2,
                65,
            )
        };
        for bad in [
            json!({"name":"intent","type":"refusal"}),
            json!({"name":"intent","type":"choice","choice":"other","confidence":1}),
            json!({"name":"intent","type":"choice","choice":"9","confidence":1}),
            json!({"name":"intent","type":"choice","choice":"1","confidence":1.2}),
            json!({"name":"wrong","type":"choice","choice":"1","confidence":1}),
        ] {
            assert!(decode(bad).is_err());
        }
        assert_eq!(
            decode(json!({"name":"intent","type":"choice","choice":"1","confidence":0.42})),
            Ok(None)
        );
        assert_eq!(
            decode(
                json!({"name":"intent","type":"choice","choice":"2","probabilities":[{"value":"2","probability":0.66}]})
            ),
            Ok(Some(1))
        );
        assert_eq!(
            verdict(
                Protocol::SystemOne,
                &json!({"answers":{"intent":{"choice":"2","probabilities":{"2":0.96}}}}),
                2,
                60
            ),
            Ok(Some(1))
        );
    }
}
