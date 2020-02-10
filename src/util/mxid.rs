pub struct MatrixId(String);

impl MatrixId {
    pub fn new(username: &str, domain: &str) -> Self {
        MatrixId(format!("@{}:{}", username, domain))
    }

    pub fn username(&self) -> &str {
        self.0.trim_start_matches('@').split(':').next().unwrap()
    }

    pub fn domain(&self) -> &str {
        self.0.split(':').nth(1).unwrap()
    }
}
