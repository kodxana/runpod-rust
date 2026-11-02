use super::*;
use std::{cell::Cell, rc::Rc};

struct Reader {
    replies: Vec<Value>,
    index: usize,
    time: Rc<Cell<f64>>,
}
impl JobReader for Reader {
    async fn get(&mut self, operation: &str, timeout: Duration) -> Result<Value, ReadFailure> {
        let reply = &self.replies[self.index];
        self.index += 1;
        assert_eq!(reply["path"], format!("ep1/{operation}/job1"));
        assert!((reply["timeout"].as_f64().unwrap() - timeout.as_secs_f64()).abs() < 1e-8);
        self.time
            .set(self.time.get() + reply["elapsed"].as_f64().unwrap_or(0.0));
        match reply["failure"].as_str() {
            Some("interrupt") => Err(ReadFailure::Interrupted),
            Some("read") => Err(ReadFailure::ReadTimeout(Error::InvalidInput(
                "read timed out".into(),
            ))),
            Some("connect") => Err(ReadFailure::ConnectTimeout(Error::InvalidInput(
                "connect timed out".into(),
            ))),
            _ => Ok(reply["response"].clone()),
        }
    }
}
struct Clock {
    time: Rc<Cell<f64>>,
    sleeps: Vec<f64>,
    interrupt: Option<usize>,
}
impl super::super::watch::Clock for Clock {
    fn now(&self) -> f64 {
        self.time.get()
    }
    async fn sleep(&mut self, seconds: f64) -> CliResult<bool> {
        self.sleeps.push(seconds);
        if self.interrupt == Some(self.sleeps.len()) {
            return Ok(true);
        }
        self.time.set(self.time.get() + seconds);
        Ok(false)
    }
}
fn normalized(text: &str, as_json: bool) -> Vec<Value> {
    if as_json {
        return serde_json::Deserializer::from_str(text)
            .into_iter::<Value>()
            .collect::<Result<_, _>>()
            .unwrap();
    }
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        return vec![value];
    }
    text.lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|_| Value::String(line.into())))
        .collect()
}
async fn verify(name: &str) {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../../../tests/fixtures/job_follow.json")).unwrap();
    let mut count = 0;
    for case in cases.iter().filter(|case| case["name"] == name) {
        count += 1;
        let time = Rc::new(Cell::new(0.0));
        let mut reader = Reader {
            replies: case["requests"].as_array().unwrap().clone(),
            index: 0,
            time: time.clone(),
        };
        let mut clock = Clock {
            time,
            sleeps: Vec::new(),
            interrupt: case["interrupt_sleep"]
                .as_u64()
                .map(|n| usize::try_from(n).unwrap()),
        };
        let options = Follow {
            endpoint_id: "ep1",
            job_id: "job1",
            stream: case["stream"].as_bool().unwrap(),
            as_json: case["as_json"].as_bool().unwrap(),
            timeout: case["timeout"].as_f64(),
        };
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let mut streams = Streams::new(&mut stdout, &mut stderr, false);
        let result = options.observe(&mut reader, &mut clock, &mut streams).await;
        let code = match result {
            Ok(()) => 0,
            Err(error) => {
                let message = if matches!(error, CliError::Aborted) {
                    format!("{error}\n")
                } else {
                    format!("Error: {error}\n")
                };
                streams.write(Destination::Err, &message).unwrap();
                error.code()
            }
        };
        assert_eq!(i64::from(code), case["code"].as_i64().unwrap(), "{case}");
        let stdout = String::from_utf8(stdout).unwrap();
        assert_eq!(
            normalized(&stdout, options.as_json),
            normalized(case["stdout"].as_str().unwrap(), options.as_json),
            "{case}"
        );
        assert_eq!(
            String::from_utf8(stderr).unwrap(),
            case["stderr"].as_str().unwrap(),
            "{case}"
        );
        assert_eq!(reader.index, reader.replies.len(), "{case}");
        let expected_sleeps: Vec<_> = case["sleeps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_f64().unwrap())
            .collect();
        assert_eq!(clock.sleeps, expected_sleeps, "{case}");
    }
    assert!(count > 0);
}
macro_rules! source_tests { ($($name:ident),* $(,)?) => {$( #[tokio::test] async fn $name() { verify(stringify!($name)).await; } )*}; }
source_tests!(
    test_job_wait_and_stream,
    test_job_stream_interrupt,
    final_output_shapes,
    failed_final_outputs,
    mixed_chunks,
    failed_stream,
    missing_chunks_fallback,
    chunk_newlines,
    read_timeout_retry,
    deadline_while_pending,
    sleep_interrupt,
    request_timeout_at_deadline,
    request_timeout_before_deadline,
    retry_deadline,
    terminal_drains_after_deadline,
    fallback_interrupt,
);
