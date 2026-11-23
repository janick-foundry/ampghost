use std::error::Error;

pub fn chain(error: &(dyn Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut current = error.source();
    while let Some(source) = current {
        text.push_str(": ");
        text.push_str(&source.to_string());
        current = source.source();
    }
    text
}
