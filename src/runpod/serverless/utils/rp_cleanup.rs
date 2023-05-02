use std::fs;

pub fn clean(folder_list: Option<Vec<&str>>) {
    fs::remove_dir_all("input_objects").unwrap_or(());
    fs::remove_dir_all("output_objects").unwrap_or(());
    fs::remove_dir_all("job_files").unwrap_or(());
    
    if fs::metadata("output.zip").is_ok() {
        fs::remove_file("output.zip").unwrap_or(());
    }

    if let Some(folders) = folder_list {
        for folder in folders {
            fs::remove_dir_all(folder).unwrap_or(());
        }
    }
}
