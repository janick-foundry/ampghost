use crate::error::StoreError;

pub trait PresetStorage: Send {
    fn list(&self) -> Result<Vec<String>, StoreError>;

    fn read(&self, key: &str) -> Result<Vec<u8>, StoreError>;

    fn write(&mut self, key: &str, bytes: &[u8]) -> Result<(), StoreError>;

    fn delete(&mut self, key: &str) -> Result<(), StoreError>;
}

pub fn validate_key(key: &str) -> Result<(), StoreError> {
    let valid = !key.is_empty()
        && key.len() <= 128
        && !key.starts_with('.')
        && key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character));
    if valid {
        Ok(())
    } else {
        Err(StoreError::InvalidKey(key.to_owned()))
    }
}

pub fn preset_key(name: &str) -> Result<String, StoreError> {
    let mut slug = String::new();
    for character in name.trim().chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        return Err(StoreError::InvalidName);
    }
    let key = format!("{slug}.json");
    validate_key(&key)?;
    Ok(key)
}
