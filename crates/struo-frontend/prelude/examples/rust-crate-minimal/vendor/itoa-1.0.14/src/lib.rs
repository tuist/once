pub struct Buffer;

impl Buffer {
    pub fn new() -> Self {
        Self
    }

    pub fn format(&mut self, value: i32) -> String {
        value.to_string()
    }
}
