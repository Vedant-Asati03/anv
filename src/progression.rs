pub fn sorted_labels_numeric(labels: &[String]) -> Vec<String> {
    let mut sorted = labels.to_vec();
    sorted.sort_by(|a, b| {
        parse_numeric_label(a)
            .partial_cmp(&parse_numeric_label(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    sorted.dedup();
    sorted
}

pub fn next_label_presorted(current: &str, sorted: &[String]) -> Option<String> {
    let pos = sorted.iter().position(|label| label == current)?;
    sorted.get(pos + 1).cloned()
}

fn parse_numeric_label(label: &str) -> f64 {
    label.parse::<f64>().unwrap_or(0.0)
}
