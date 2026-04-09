use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct StreamOption {
    pub provider: String,
    pub url: String,
    pub quality_label: String,
    pub quality_rank: i32,
    pub is_hls: bool,
    pub headers: HashMap<String, String>,
    pub subtitle: Option<String>,
}

impl StreamOption {
    pub fn label(&self) -> String {
        let kind = if self.is_hls { "HLS" } else { "MP4" };
        format!("{} {} ({})", self.provider, self.quality_label, kind)
    }
}

#[derive(Debug, Clone)]
pub struct Page {
    pub url: String,
    pub headers: HashMap<String, String>,
}
