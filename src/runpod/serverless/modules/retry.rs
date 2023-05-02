use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tokio::time::sleep;
use rand::Rng;

pub fn retry<F, Fut, T, E>(
    mut f: F,
    max_attempts: usize,
    base_delay: Duration,
    max_delay: Duration,
) -> impl Future<Output = Result<T, E>>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    async move {
        let mut attempt = 1;

        loop {
            match f().await {
                Ok(result) => return Ok(result),
                Err(err) => {
                    if attempt >= max_attempts {
                        return Err(err);
                    }

                    // Calculate delay using exponential backoff with random jitter
                    let delay = base_delay * (2u32.pow(attempt - 1) as u64);
                    let delay = delay.min(max_delay);
                    let jitter = rand::thread_rng().gen_range(0.5..1.5);
                    let delay = Duration::from_secs_f64(delay.as_secs_f64() * jitter);

                    // Wait for the delay before retrying
                    sleep(delay).await;
                    attempt += 1;
                }
            }
        }
    }
}

// Example usage:

async fn error_prone_function() -> Result<(), &'static str> {
    Err("An error occurred")
}

#[tokio::main]
async fn main() {
    let max_attempts = 3;
    let base_delay = Duration::from_secs(1);
    let max_delay = Duration::from_secs(3);

    match retry(|| error_prone_function(), max_attempts, base_delay, max_delay).await {
        Ok(_) => println!("Success!"),
        Err(err) => println!("Error: {}", err),
    }
}
