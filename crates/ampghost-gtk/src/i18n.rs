use gettextrs::{bindtextdomain, gettext, textdomain};

const DOMAIN: &str = "tsuba";
const LOCALE_DIR: &str = "/usr/share/locale";

pub fn init() {
    if let Err(error) = bindtextdomain(DOMAIN, LOCALE_DIR) {
        tracing::warn!(%error, "bindtextdomain failed");
    }
    if let Err(error) = textdomain(DOMAIN) {
        tracing::warn!(%error, "textdomain failed");
    }
}

pub fn tr(message: &str) -> String {
    gettext(message)
}

pub fn trf(message: &str, args: &[(&str, &str)]) -> String {
    let mut text = gettext(message);
    for (name, value) in args {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}
