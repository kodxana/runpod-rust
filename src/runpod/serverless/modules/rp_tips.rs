use std::mem;
use log::{info, warn};

pub fn check_return_size<T>(return_body: &T) {
    let size_bytes = mem::size_of_val(return_body);
    let size_mb = (size_bytes as f64) / 1_000_000.0;

    if size_mb > 20.0 {
        warn!(
            "Your return body is {:.2} MB which exceeds the 20 MB limit. \
            Consider using S3 upload and returning the object's URL instead.",
            size_mb
        );
    } else {
        info!("Return body size: {:.2} MB", size_mb);
    }
}
