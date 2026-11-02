use super::CliResult;
use std::io::Write;

#[derive(Clone, Copy)]
pub(super) enum Destination {
    Out,
    Err,
}

pub(super) struct Streams<'a> {
    pub out: &'a mut (dyn Write + Send),
    pub err: &'a mut (dyn Write + Send),
    pub terminal: bool,
    frame_lines: usize,
}
impl<'a> Streams<'a> {
    pub fn new(
        out: &'a mut (dyn Write + Send),
        err: &'a mut (dyn Write + Send),
        terminal: bool,
    ) -> Self {
        Self {
            out,
            err,
            terminal,
            frame_lines: 0,
        }
    }
    pub fn write(&mut self, destination: Destination, text: &str) -> CliResult<()> {
        let stream = match destination {
            Destination::Out => &mut self.out,
            Destination::Err => &mut self.err,
        };
        stream
            .write_all(text.as_bytes())
            .map_err(crate::Error::from)?;
        stream.flush().map_err(crate::Error::from)?;
        Ok(())
    }
    pub fn frame(&mut self, text: &str) -> CliResult<()> {
        if self.terminal && self.frame_lines > 0 {
            dialoguer::console::Term::stdout()
                .clear_last_lines(self.frame_lines)
                .map_err(crate::Error::from)?;
        }
        self.write(Destination::Out, text)?;
        self.frame_lines = frame_height(
            text,
            usize::from(dialoguer::console::Term::stdout().size().1),
        );
        Ok(())
    }
}
fn frame_height(text: &str, columns: usize) -> usize {
    use unicode_width::UnicodeWidthStr;
    text.lines()
        .map(|line| line.width().max(1).div_ceil(columns.max(1)))
        .sum()
}

#[cfg(test)]
mod tests {
    #[test]
    fn frame_height_counts_wrapped_terminal_rows() {
        assert_eq!(
            super::frame_height(&format!("WORKER GPU\nw1 {}\n\n", "界".repeat(40)), 80),
            4
        );
        assert_eq!(
            super::frame_height(&format!("{}\n", "e\u{301}".repeat(80)), 80),
            1
        );
        assert_eq!(super::frame_height(&format!("{}\n", "a".repeat(81)), 80), 2);
    }
}
