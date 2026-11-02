use crate::{Value, control::python_string};
use std::{collections::BTreeMap, fmt::Write};
#[derive(Default)]
pub(super) struct Progress {
    rows: Vec<Row>,
    current: Option<usize>,
    package_count: u64,
    upload_total: u64,
    pub(super) endpoints: BTreeMap<String, String>,
}
struct Row {
    name: String,
    label: String,
    detail: String,
    done: bool,
}
impl Progress {
    pub(super) fn event(&mut self, event: &str, args: &[Value]) {
        match event {
            "phase" => {
                self.close();
                let name = args.first().map(python_string).unwrap_or_default();
                let label = match name.as_str() {
                    "compile" => "compiling worker",
                    "vendor" => "resolving dependencies",
                    "package" => "packaging artifact",
                    "upload" => "uploading build",
                    "endpoints" => "reconciling endpoints",
                    other => other,
                }
                .to_owned();
                self.current = Some(self.rows.len());
                self.rows.push(Row {
                    name,
                    label,
                    detail: args.get(1).map(python_string).unwrap_or_default(),
                    done: false,
                });
            }
            "vendor_progress" => {
                if let Some(index) = self.current {
                    self.package_count = args.first().and_then(Value::as_u64).unwrap_or(0);
                    self.rows[index].detail = format!(
                        "{} ({})",
                        args.get(1).map(python_string).unwrap_or_default(),
                        self.package_count
                    );
                }
            }
            "upload_progress" => {
                if let Some(index) = self.current {
                    let sent = args.first().and_then(Value::as_u64).unwrap_or(0);
                    self.upload_total = args.get(1).and_then(Value::as_u64).unwrap_or(0);
                    let fraction = if self.upload_total == 0 {
                        0.0
                    } else {
                        sent as f64 / self.upload_total as f64
                    };
                    self.rows[index].detail = format!(
                        "{} {:.1} / {:.1} MB",
                        bar(fraction),
                        sent as f64 / 1_048_576.0,
                        self.upload_total as f64 / 1_048_576.0
                    );
                }
            }
            "endpoint_ready" => {
                if let (Some(name), Some(id)) = (
                    args.first().and_then(Value::as_str),
                    args.get(1).and_then(Value::as_str),
                ) {
                    self.endpoints.insert(name.into(), id.into());
                }
            }
            _ => (),
        }
    }
    pub(super) fn close(&mut self) {
        if let Some(index) = self.current.take() {
            let row = &mut self.rows[index];
            row.done = true;
            row.detail = match row.name.as_str() {
                "vendor" if self.package_count > 0 => format!("{} packages", self.package_count),
                "upload" if self.upload_total > 0 => {
                    format!("{:.1} MB", self.upload_total as f64 / 1_048_576.0)
                }
                _ => String::new(),
            };
        }
    }
    pub(super) fn render(&self, frame: usize) -> String {
        let mut output = String::new();
        let frames = [
            '\u{2801}', '\u{2802}', '\u{2804}', '\u{2840}', '\u{2880}', '\u{2820}', '\u{2810}',
            '\u{2808}',
        ];
        for row in &self.rows {
            let marker = if row.done {
                '\u{2713}'
            } else {
                frames[frame % frames.len()]
            };
            let detail = if row.detail.is_empty() {
                String::new()
            } else {
                format!(" {}", row.detail)
            };
            writeln!(output, "{marker} {}{detail}", row.label).expect("write to String");
        }
        output
    }
}
pub(super) fn bar(fraction: f64) -> String {
    let filled = (fraction.clamp(0.0, 1.0) * 22.0) as usize;
    format!(
        "{}{}",
        "\u{2501}".repeat(filled),
        "\u{2500}".repeat(22 - filled)
    )
}
