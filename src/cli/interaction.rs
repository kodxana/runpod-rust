use super::{CliError, CliResult};
use std::io::IsTerminal;
use unicode_width::UnicodeWidthStr;

pub(super) trait Interaction {
    fn interactive(&self) -> bool;
    fn columns(&self) -> usize;
    fn select(&mut self, message: &str, labels: &[String]) -> CliResult<Option<usize>>;
    fn confirm_prompt(&mut self, message: &str) -> CliResult<bool>;
    fn text_prompt(&mut self, message: &str) -> CliResult<String>;
    fn hidden_prompt(&mut self, message: &str) -> CliResult<String>;
    fn edit(&mut self, text: &str) -> CliResult<Option<String>>;
    fn retry_edit(&mut self) -> CliResult<bool>;

    fn pick(
        &mut self,
        message: &str,
        choices: &[(String, String)],
        missing: &str,
    ) -> CliResult<String> {
        if !self.interactive() {
            return Err(CliError::Usage(missing.into()));
        }
        if choices.is_empty() {
            return Err(CliError::Usage(format!("{missing} (none found)")));
        }
        let width = self.columns().saturating_sub(message.width() + 3).max(1);
        let labels: Vec<_> = choices
            .iter()
            .map(|(_, label)| dialoguer::console::truncate_str(label, width, "…").into_owned())
            .collect();
        let index = self.select(message, &labels)?.ok_or(CliError::Aborted)?;
        choices
            .get(index)
            .map(|(id, _)| id.clone())
            .ok_or_else(|| CliError::Usage("invalid selection".into()))
    }
    fn confirm(&mut self, message: &str, yes: bool) -> CliResult<()> {
        if yes {
            return Ok(());
        }
        if !self.interactive() {
            return Err(CliError::Usage(
                "pass --yes to confirm in a non-interactive shell".into(),
            ));
        }
        if self.confirm_prompt(message)? {
            Ok(())
        } else {
            Err(CliError::Aborted)
        }
    }
}

pub(super) struct Terminal;
impl Interaction for Terminal {
    fn hidden_prompt(&mut self, message: &str) -> CliResult<String> {
        let term = dialoguer::console::Term::stdout();
        term.write_str(&format!("{message}: "))
            .map_err(crate::Error::from)?;
        term.flush().map_err(crate::Error::from)?;
        // The raw key API returns Ctrl-C instead of terminating the Windows process.
        let result = hidden_line(|| term.read_key_raw());
        term.write_line("").map_err(crate::Error::from)?;
        result
    }
    fn text_prompt(&mut self, message: &str) -> CliResult<String> {
        Ok(dialoguer::Input::new()
            .with_prompt(message)
            .interact_text_on(&dialoguer::console::Term::stdout())?)
    }
    fn edit(&mut self, text: &str) -> CliResult<Option<String>> {
        super::editor::edit(text)
    }
    fn retry_edit(&mut self) -> CliResult<bool> {
        Ok(dialoguer::Confirm::new()
            .with_prompt("re-open the editor?")
            .default(true)
            .wait_for_newline(true)
            .interact_on_opt(&dialoguer::console::Term::stdout())?
            .unwrap_or(false))
    }
    fn interactive(&self) -> bool {
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
    }
    fn columns(&self) -> usize {
        usize::from(dialoguer::console::Term::stdout().size().1)
    }
    fn select(&mut self, message: &str, labels: &[String]) -> CliResult<Option<usize>> {
        Ok(dialoguer::FuzzySelect::new()
            .with_prompt(message)
            .items(labels)
            .interact_on_opt(&dialoguer::console::Term::stdout())?)
    }
    fn confirm_prompt(&mut self, message: &str) -> CliResult<bool> {
        Ok(dialoguer::Confirm::new()
            .with_prompt(message)
            .default(false)
            .wait_for_newline(true)
            .interact_on_opt(&dialoguer::console::Term::stdout())?
            .unwrap_or(false))
    }
}

fn hidden_line(
    mut read: impl FnMut() -> std::io::Result<dialoguer::console::Key>,
) -> CliResult<String> {
    use dialoguer::console::Key;
    let mut value = String::new();
    loop {
        let key = read().map_err(|error| {
            if error.kind() == std::io::ErrorKind::Interrupted {
                CliError::Aborted
            } else {
                crate::Error::from(error).into()
            }
        })?;
        match key {
            Key::CtrlC | Key::Char('\u{4}') => return Err(CliError::Aborted),
            Key::Enter if !value.is_empty() => return Ok(value),
            Key::Backspace => {
                value.pop();
            }
            Key::Char(ch) if !ch.is_control() => value.push(ch),
            _ => {}
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    #[test]
    fn hidden_input_supports_unicode_backspace_and_rejects_empty_lines() {
        use dialoguer::console::Key;
        let mut keys = [
            Key::Enter,
            Key::Backspace,
            Key::Char('é'),
            Key::Char('界'),
            Key::Backspace,
            Key::Char('😀'),
            Key::ArrowLeft,
            Key::Enter,
        ]
        .into_iter();
        assert_eq!(
            hidden_line(|| Ok(keys.next().expect("key sequence ends with Enter"))).unwrap(),
            "é😀"
        );
    }

    #[test]
    fn hidden_input_interrupts_without_reading_further() {
        use dialoguer::console::Key;
        for key in [Key::CtrlC, Key::Char('\u{4}')] {
            let mut keys = [Key::Char('x'), key, Key::Enter].into_iter();
            assert!(matches!(
                hidden_line(|| Ok(keys.next().unwrap())),
                Err(CliError::Aborted)
            ));
            assert_eq!(keys.next(), Some(Key::Enter));
        }
        assert!(matches!(
            hidden_line(|| Err(std::io::ErrorKind::Interrupted.into())),
            Err(CliError::Aborted)
        ));
    }

    pub(crate) struct Fake {
        pub interactive: bool,
        pub columns: usize,
        pub selected: Option<usize>,
        pub answer: bool,
        pub selections: Vec<(String, Vec<String>)>,
        pub confirmations: Vec<String>,
        pub edits: std::collections::VecDeque<Option<String>>,
        pub edit_inputs: Vec<String>,
        pub retries: std::collections::VecDeque<bool>,
        pub texts: std::collections::VecDeque<String>,
        pub prompts: Vec<(String, bool)>,
    }
    impl Default for Fake {
        fn default() -> Self {
            Self {
                interactive: true,
                columns: 80,
                selected: Some(0),
                answer: true,
                selections: Vec::new(),
                confirmations: Vec::new(),
                edits: std::collections::VecDeque::new(),
                edit_inputs: Vec::new(),
                retries: std::collections::VecDeque::new(),
                texts: std::collections::VecDeque::new(),
                prompts: Vec::new(),
            }
        }
    }
    impl Interaction for Fake {
        fn text_prompt(&mut self, message: &str) -> CliResult<String> {
            self.prompts.push((message.into(), false));
            self.texts.pop_front().ok_or(CliError::Aborted)
        }
        fn hidden_prompt(&mut self, message: &str) -> CliResult<String> {
            self.prompts.push((message.into(), true));
            self.texts.pop_front().ok_or(CliError::Aborted)
        }
        fn edit(&mut self, text: &str) -> CliResult<Option<String>> {
            self.edit_inputs.push(text.into());
            Ok(self.edits.pop_front().expect("configured editor response"))
        }
        fn retry_edit(&mut self) -> CliResult<bool> {
            Ok(self.retries.pop_front().unwrap_or(false))
        }
        fn interactive(&self) -> bool {
            self.interactive
        }
        fn columns(&self) -> usize {
            self.columns
        }
        fn select(&mut self, message: &str, labels: &[String]) -> CliResult<Option<usize>> {
            self.selections.push((message.into(), labels.to_vec()));
            Ok(self.selected)
        }
        fn confirm_prompt(&mut self, message: &str) -> CliResult<bool> {
            self.confirmations.push(message.into());
            Ok(self.answer)
        }
    }
    #[test]
    fn test_pick_requires_tty() {
        let mut ui = Fake {
            interactive: false,
            ..Fake::default()
        };
        assert_eq!(
            ui.pick("pick", &[("a".into(), "a".into())], "pass an ID")
                .unwrap_err()
                .to_string(),
            "pass an ID"
        );
        assert_eq!(ui.selections, Vec::<(String, Vec<String>)>::new());
    }
    #[test]
    fn test_pick_with_no_choices() {
        assert_eq!(
            Fake::default()
                .pick("pick", &[], "pass an ID")
                .unwrap_err()
                .to_string(),
            "pass an ID (none found)"
        );
    }
    #[test]
    fn test_pick_uses_fuzzy_prompt() {
        let mut ui = Fake {
            selected: Some(1),
            ..Fake::default()
        };
        assert_eq!(
            ui.pick(
                "pick",
                &[("a".into(), "first".into()), ("b".into(), "second".into())],
                "x"
            )
            .unwrap(),
            "b"
        );
        assert_eq!(
            ui.selections,
            vec![("pick".into(), vec!["first".into(), "second".into()])]
        );
    }
    #[test]
    fn test_confirm() {
        let mut ui = Fake {
            interactive: false,
            ..Fake::default()
        };
        ui.confirm("sure?", true).unwrap();
        assert_eq!(ui.confirmations, Vec::<String>::new());
        assert!(
            ui.confirm("sure?", false)
                .unwrap_err()
                .to_string()
                .contains("--yes")
        );
        ui.interactive = true;
        ui.confirm("sure?", false).unwrap();
        assert_eq!(ui.confirmations, vec!["sure?"]);
        ui.answer = false;
        assert!(matches!(ui.confirm("sure?", false), Err(CliError::Aborted)));
    }
    #[test]
    fn test_pick_truncates_labels_to_terminal_cells() {
        for message in ["volume", "pod", "endpoint", "区域"] {
            for columns in [20, 40] {
                let mut ui = Fake {
                    columns,
                    selected: Some(1),
                    ..Fake::default()
                };
                let choices = vec![
                    (
                        "first-id".into(),
                        "capy-test-example-network  EU-RO-1  10 GB".into(),
                    ),
                    ("second-id".into(), "训练资源".repeat(12)),
                ];
                assert_eq!(ui.pick(message, &choices, "x").unwrap(), "second-id");
                for label in &ui.selections[0].1 {
                    assert!(label.ends_with('…'));
                    assert!(format!("? {message} {label}").width() <= columns);
                }
            }
        }
    }
}
