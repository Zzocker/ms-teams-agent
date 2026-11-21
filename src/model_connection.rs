//! Ask Me's saved inference endpoint and its Settings connection check.
use crate::*;

const ENDPOINT: &str = "http://127.0.0.1:11435/v1";
const MODEL: &str = "qwen3.5:2b-q4_K_M";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Config {
    pub(crate) endpoint: String,
    pub(crate) model: String,
}

impl Config {
    pub(crate) fn new(endpoint: &str, model: &str) -> Result<Self, &'static str> {
        let endpoint = endpoint.trim().trim_end_matches('/');
        let valid_endpoint = endpoint.parse::<ureq::http::Uri>().ok().is_some_and(|uri| {
            matches!(uri.scheme_str(), Some("http" | "https"))
                && uri.host().is_some()
                && uri.authority().is_some_and(|authority| !authority.as_str().contains('@'))
                && uri.query().is_none()
                && !endpoint.contains('#')
        });
        if !valid_endpoint {
            return Err("Enter an HTTP or HTTPS base URL without credentials, a query, or a fragment.");
        }
        let model = model.trim();
        if model.is_empty() || model.chars().any(char::is_control) {
            return Err("Enter the model ID shown by your server.");
        }
        Ok(Self { endpoint: endpoint.into(), model: model.into() })
    }
}

pub(crate) struct Connection {
    pub(crate) saved: Option<Config>,
    pub(crate) worker: Latest<(Config, Result<(), String>)>,
    endpoint: TextEdit,
    model: TextEdit,
    status: Option<Result<(), String>>,
}

impl Default for Connection {
    fn default() -> Self {
        Self {
            saved: None,
            worker: Latest::default(),
            endpoint: TextEdit::new(ENDPOINT),
            model: TextEdit::new(MODEL),
            status: None,
        }
    }
}

impl Connection {
    pub(crate) fn restore(&mut self, config: Config) {
        self.endpoint.replace_text(config.endpoint.clone());
        self.model.replace_text(config.model.clone());
        self.saved = Some(config);
    }

    fn draft(&self) -> Result<Config, &'static str> {
        Config::new(&self.endpoint.text, &self.model.text)
    }
}

fn probe(config: &Config) -> Result<(), String> {
    complete(config, serde_json::json!({
        "model": config.model,
        "messages": [{"role": "user", "content": "Reply with OK."}],
        "max_tokens": 24,
        "stream": false
    }), 30).map(drop)
}

pub(crate) fn complete(config: &Config, body: serde_json::Value, timeout_secs: u64) -> Result<String, String> {
    let mut response = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(timeout_secs)))
        .max_redirects(0)
        .build().new_agent()
        .post(&format!("{}/chat/completions", config.endpoint))
        .send_json(body).map_err(|error| format!("Couldn’t reach the model: {error}"))?;
    let result: serde_json::Value = response.body_mut().with_config().limit(64 * 1024)
        .read_json().map_err(|_| "The server didn’t return a chat completion.".to_owned())?;
    result["choices"][0]["message"]["content"].as_str()
        .filter(|text| !text.trim().is_empty()).map(str::to_owned)
        .ok_or_else(|| "The server returned no answer. Check the model ID and base URL.".into())
}

fn edited(s: &mut State, _: &mut Tx) {
    s.model_connection.worker.cancel();
    s.model_connection.status = None;
}

fn test_and_save(s: &mut State, _: &mut Tx) {
    let connection = &mut s.model_connection;
    if connection.worker.is_pending() { return; }
    match connection.draft() {
        Ok(config) => {
            connection.status = None;
            connection.worker.request(config, |config| {
                let result = probe(&config);
                (config, result)
            });
        }
        Err(error) => connection.status = Some(Err(error.into())),
    }
}

pub(crate) fn poll(s: &mut State) -> bool {
    let Some((config, result)) = s.model_connection.worker.take() else { return false; };
    if s.model_connection.draft().as_ref().ok() != Some(&config) { return false; }
    let result = result.and_then(|()| settings::save_model(s, &config)
        .map_err(|error| format!("Model answered, but settings couldn’t be saved: {error}")));
    if result.is_ok() { s.model_connection.restore(config); }
    s.model_connection.status = Some(result);
    true
}

pub(crate) fn panel(s: &State) -> El {
    let t = tok::theme(dark());
    let accent = tok::accent(0, dark());
    let connection = &s.model_connection;
    let pending = connection.worker.is_pending();
    let field = |key, label, draft, binding: fn(&mut State) -> &mut TextEdit| {
        col().w_full().min_w(0.0).gap(4.0)
            .child(text(label).font(type_::LABEL).color(t.text))
            .child(text_input(draft, type_::BODY).key(key).label(label)
                .w_full().min_w(0.0).min_h(36.0).px(8.0).py(7.0).radius(6.0)
                .fill(t.bg).border(1.0, t.separator).color(t.text)
                .caret(t.text, accent.alpha(0.3)).on_edit(binding)
                .on_input_change(edited).on_submit(test_and_save))
    };
    let status = if pending {
        Some(("Testing connection…", "status"))
    } else {
        match &connection.status {
            Some(Ok(())) => Some((if s.settings_file.is_some() { "Connected. Model settings saved." } else { "Connected for this session." }, "status")),
            Some(Err(error)) => Some((error.as_str(), "alert")),
            None if connection.saved.is_some() && connection.draft().as_ref().ok() == connection.saved.as_ref() =>
                Some(("Saved connection", "status")),
            None => None,
        }
    };
    col().key("ask-me-model-settings").grow(1.0).min_w(0.0).min_h(0.0).h_full()
        .padding(20.0).gap(12.0).scroll(ScrollSpec { stick_bottom: false })
        .child(text("Ask Me").role("heading").font(type_::TITLE).color(t.text))
        .child(col().w_full().min_w(0.0).padding(12.0).gap(8.0).radius(8.0).border(1.0, t.separator)
            .child(text("Model connection").role("heading").font(type_::TITLE).color(t.text))
            .child(field("ask-me-model-endpoint", "API base URL", &connection.endpoint, |s| &mut s.model_connection.endpoint))
            .child(field("ask-me-model-id", "Model ID", &connection.model, |s| &mut s.model_connection.model))
            .child(row().w_full().flex_wrap().items_center().gap(8.0)
                .child(el().key("ask-me-model-connect").role("button").label("Test and save").focusable(true)
                    .disabled(pending).min_h(36.0).px(12.0).center().radius(6.0)
                    .fill(if pending { t.control_off } else { accent })
                    .cursor(if pending { rune::Cursor::Default } else { rune::Cursor::Pointer })
                    .on_press_tx(test_and_save)
                    .child(text("Test and save").font(type_::LABEL).color(if pending { t.text_secondary } else { t.bg })))
                .children(status.map(|(message, role)| {
                    text(message).key("ask-me-model-status").role(role).font(type_::LABEL)
                        .color(t.text_secondary).wrap().shrink().min_w(0.0)
                }))))
}
