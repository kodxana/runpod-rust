use super::{
    CliError, CliResult,
    interaction::{Interaction, Terminal},
    streams::{Destination, Streams},
};
use crate::{
    apps::{
        AppsApiClient,
        auth::{self, EmailLoginClient},
    },
    credentials::{CredentialStore, Overwrite},
};
use clap::{Arg, ArgAction, ArgMatches, Command};
use std::{
    future::{Future, ready},
    process::Stdio,
};

pub(super) fn command() -> Command {
    Command::new("login")
        .about("Authenticate with Runpod and store the API key")
        .long_about("Authenticate with Runpod and store the API key.\n\nOpens the Runpod console for browser approval by default; pass --api-key to store a key directly. Use --agent --email ADDRESS to receive a verification code by email and enter it at the prompt in an interactive terminal. Keep this process running until sign-in completes; the code must be entered in the same process that requested it.")
        .args_override_self(true)
        .arg(Arg::new("api-key").long("api-key").allow_hyphen_values(true).help("Store this API key directly"))
        .arg(Arg::new("no-open").long("no-open").action(ArgAction::SetTrue).help("Print the URL without opening a browser"))
        .arg(Arg::new("profile").long("profile").default_value("default").allow_hyphen_values(true).help("Credential profile"))
        .arg(Arg::new("agent").long("agent").action(ArgAction::SetTrue).help("Sign in with an emailed code; requires an interactive terminal"))
        .arg(Arg::new("email").long("email").allow_hyphen_values(true).help("Email address for --agent sign-in"))
}

enum Mode {
    Direct(String),
    Agent(String),
    Browser { no_open: bool },
}
pub(super) struct Options {
    mode: Mode,
    profile: String,
}
impl Options {
    pub(super) fn read(args: &ArgMatches) -> CliResult<Self> {
        let agent = args.get_flag("agent");
        let key = args.get_one::<String>("api-key");
        let email = args.get_one::<String>("email");
        // Check presence before truthiness: even --api-key '' conflicts with --agent.
        if agent && key.is_some() {
            return Err(CliError::Usage(
                "--agent cannot be combined with --api-key".into(),
            ));
        }
        if agent && email.is_none_or(|v| v.trim().is_empty()) {
            return Err(CliError::Usage("--agent requires --email".into()));
        }
        if email.is_some() && !agent {
            return Err(CliError::Usage("--email requires --agent".into()));
        }
        let mode = if let Some(key) = key.filter(|key| !key.is_empty()) {
            Mode::Direct(key.clone())
        } else if agent {
            Mode::Agent(email.map(|s| s.trim().to_owned()).unwrap_or_default())
        } else {
            Mode::Browser {
                no_open: args.get_flag("no-open"),
            }
        };
        Ok(Self {
            mode,
            profile: args
                .get_one::<String>("profile")
                .cloned()
                .unwrap_or_default(),
        })
    }

    pub(super) async fn run(self, streams: &mut Streams<'_>) -> CliResult<()> {
        let store = CredentialStore::user_default()?;
        let mut save =
            |key: &str, profile: &str| store.set_credentials(key, profile, Overwrite::Allow);
        let mut authentication = NativeAuth {
            base_url: std::env::var("RUNPOD_API_BASE_URL")
                .unwrap_or_else(|_| "https://api.runpod.io".into()),
        };
        let mut terminal = Terminal;
        let mut launch = launch_browser;
        tokio::select! {
            result=self.execute(&mut authentication,&mut save,&mut terminal,streams,&mut launch) => result,
            signal=tokio::signal::ctrl_c() => {
                signal.map_err(crate::Error::from)?;
                Err(CliError::Aborted)
            }
        }
    }

    async fn execute(
        self,
        authentication: &mut impl Authentication,
        save: &mut (impl FnMut(&str, &str) -> crate::Result<()> + Send),
        ui: &mut (impl Interaction + Send),
        streams: &mut Streams<'_>,
        launch: &mut (impl FnMut(&str) + Send),
    ) -> CliResult<()> {
        let direct = matches!(self.mode, Mode::Direct(_));
        let key = match self.mode {
            Mode::Direct(key) => key,
            Mode::Browser { no_open } => {
                let mut show = |url: &str| {
                    write!(streams.out, "\n authorize in your browser:\n {url}\n\n")?;
                    streams.out.flush()?;
                    if !no_open {
                        launch(url);
                    }
                    Ok(())
                };
                authentication.browser(&mut show).await?
            }
            Mode::Agent(email) => {
                if !ui.interactive() {
                    return Err(CliError::Usage(
                        "--agent requires an interactive terminal".into(),
                    ));
                }
                let mut prompted = false;
                let mut input_error = None;
                let mut read = || match read_code(ui, streams, &mut prompted) {
                    Ok(code) => Ok(code),
                    Err(error) => {
                        input_error = Some(error);
                        Err(crate::Error::InvalidInput("login input failed".into()))
                    }
                };
                let result = authentication.agent(&email, &mut read).await;
                if let Some(error) = input_error {
                    return Err(error);
                }
                result?
            }
        };
        save(&key, &self.profile)?;
        streams.write(
            Destination::Out,
            if direct {
                "✓ credentials saved to ~/.runpod/config.toml\n"
            } else {
                "✓ logged in, credentials saved to ~/.runpod/config.toml\n"
            },
        )
    }
}

fn read_code(
    ui: &mut impl Interaction,
    streams: &mut Streams<'_>,
    prompted: &mut bool,
) -> CliResult<String> {
    if *prompted {
        streams.write(Destination::Err, "Verification code rejected. Try again.\n")?;
    } else {
        streams.write(
            Destination::Out,
            "A verification code was sent to your email.\n",
        )?;
        *prompted = true;
    }
    loop {
        let code = ui.hidden_prompt("Verification code")?;
        let code = code.trim();
        if code.len() == 6 && code.bytes().all(|c| c.is_ascii_digit()) {
            return Ok(code.into());
        }
        streams.write(
            Destination::Err,
            "Enter the six-digit code from your email.\n",
        )?;
    }
}

trait Authentication: Send {
    fn browser(
        &mut self,
        show: &mut auth::LoginUrlObserver<'_>,
    ) -> impl Future<Output = crate::Result<String>> + Send;
    fn agent(
        &mut self,
        email: &str,
        read: &mut (dyn FnMut() -> crate::Result<String> + Send),
    ) -> impl Future<Output = crate::Result<String>> + Send;
}
struct NativeAuth {
    base_url: String,
}
impl Authentication for NativeAuth {
    async fn browser(&mut self, show: &mut auth::LoginUrlObserver<'_>) -> crate::Result<String> {
        let api = AppsApiClient::anonymous(&self.base_url)?;
        auth::browser_login(&api, &auth::BrowserLoginOptions::default(), Some(show)).await
    }
    async fn agent(
        &mut self,
        email: &str,
        read: &mut (dyn FnMut() -> crate::Result<String> + Send),
    ) -> crate::Result<String> {
        EmailLoginClient::new(&self.base_url)?
            .agent_login(email, || ready(read()))
            .await
    }
}

fn launch_browser(url: &str) {
    #[cfg(windows)]
    let mut command = {
        // Keep the launcher helper from opening a console window.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut command = tokio::process::Command::new("rundll32.exe");
        command
            .arg("url.dll,FileProtocolHandler")
            .creation_flags(CREATE_NO_WINDOW);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = tokio::process::Command::new("open");
    #[cfg(not(any(windows, target_os = "macos")))]
    let mut command = tokio::process::Command::new("xdg-open");
    // Click ignores the launcher's return status. The printed URL remains usable.
    if let Ok(mut child) = command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
    }
}

#[cfg(test)]
#[path = "login_tests.rs"]
mod tests;
