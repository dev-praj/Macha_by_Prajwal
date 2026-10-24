pub fn t(message: &str) -> String {
    message.to_string()
}

pub fn tf(template: &str, values: &[(&str, &str)]) -> String {
    values.iter().fold(template.to_string(), |text, (key, value)| {
        text.replace(&format!("{{{key}}}"), value)
    })
}
