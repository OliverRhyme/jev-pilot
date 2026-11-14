//! The device a run drives a browser through.
//!
//! [`BrowserDevice`] implements [`Device`] over any [`BrowserEngine`], so the
//! pilot drives a browser exactly as it drives a phone: observe returns a
//! snapshot, perform carries out a command, and outcome says what the last one
//! did. The engine reads the page; the Browser platform decides what of it is
//! offered.

use crate::act::{DialogReply, SystemAct};
use crate::browser::engine::{BrowserCommand, BrowserEngine, DialogAnswer};
use crate::device::{Command, Device};
use crate::platform::browser::Browser;
use crate::snapshot::Snapshot;

/// The engine's command for one of the pilot's, or `None` for a gesture a
/// browser has no equivalent for.
///
/// A phone gesture is refused rather than approximated: a long press turned
/// into a click is an action nobody chose.
#[must_use]
pub fn browser_command(command: &Command) -> Option<BrowserCommand> {
    Some(match command {
        Command::Press { at, node } => BrowserCommand::Press {
            at: *at,
            node: node.clone(),
        },
        Command::Fill { at, node, text } => BrowserCommand::Fill {
            at: *at,
            node: node.clone(),
            text: text.clone(),
        },
        Command::System(SystemAct::Submit) => BrowserCommand::Submit,
        Command::System(SystemAct::Back) => BrowserCommand::Back,
        Command::Scroll(direction) => BrowserCommand::Scroll(*direction),
        Command::Navigate(url) => BrowserCommand::Navigate(url.clone()),
        Command::SwitchTab(tab) => BrowserCommand::SwitchTab(tab.clone()),
        Command::Dialog(DialogReply::Accept { words }) => {
            BrowserCommand::Dialog(DialogAnswer::Accept(words.clone()))
        }
        Command::Dialog(DialogReply::Dismiss) => BrowserCommand::Dialog(DialogAnswer::Dismiss),
        Command::Settle => BrowserCommand::Settle,
        Command::Tap(_)
        | Command::TypeText { .. }
        | Command::DoubleTap(_)
        | Command::LongPress(_)
        | Command::SwipeFrom { .. }
        | Command::Peek(_)
        | Command::Launch(_)
        | Command::System(SystemAct::Home | SystemAct::AppSwitcher) => return None,
    })
}

/// What can go wrong driving a browser as a device.
#[derive(Debug)]
#[non_exhaustive]
pub enum BrowserDeviceError<E> {
    /// The engine failed.
    Engine(E),
    /// The command is a phone gesture with no browser equivalent.
    Unsupported(Command),
}

impl<E: core::fmt::Display> core::fmt::Display for BrowserDeviceError<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Engine(e) => e.fmt(f),
            Self::Unsupported(command) => {
                write!(f, "a browser cannot carry out {command:?}")
            }
        }
    }
}

impl<E: core::error::Error + 'static> core::error::Error for BrowserDeviceError<E> {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Engine(e) => Some(e),
            Self::Unsupported(_) => None,
        }
    }
}

/// A browser, driven as a device.
#[derive(Debug)]
pub struct BrowserDevice<E> {
    engine: E,
    platform: Browser,
}

impl<E: BrowserEngine> BrowserDevice<E> {
    /// Drive the browser behind this engine, offering what the default Browser
    /// platform offers.
    pub fn new(engine: E) -> Self {
        Self::with_platform(engine, Browser::default())
    }

    /// Drive it with a differently configured Browser platform.
    pub fn with_platform(engine: E, platform: Browser) -> Self {
        Self { engine, platform }
    }

    /// The engine.
    pub fn engine(&self) -> &E {
        &self.engine
    }

    /// The engine, to change.
    pub fn engine_mut(&mut self) -> &mut E {
        &mut self.engine
    }

    /// The platform that reads the engine's pages.
    pub fn platform(&self) -> &Browser {
        &self.platform
    }
}

impl<E: BrowserEngine> Device for BrowserDevice<E>
where
    E::Error: 'static,
{
    type Error = BrowserDeviceError<E::Error>;

    fn observe(&mut self) -> Result<Snapshot, Self::Error> {
        let read = self.engine.read().map_err(BrowserDeviceError::Engine)?;
        let quiet = self
            .engine
            .quiet_for_ms()
            .map(|ms| u32::try_from(ms).unwrap_or(u32::MAX));
        Ok(self.platform.snapshot_from(&read).quiet_for(quiet))
    }

    fn perform(&mut self, command: &Command) -> Result<(), Self::Error> {
        let browser = browser_command(command)
            .ok_or_else(|| BrowserDeviceError::Unsupported(command.clone()))?;
        self.engine
            .perform(&browser)
            .map_err(BrowserDeviceError::Engine)
    }

    fn outcome(&mut self) -> Option<Box<str>> {
        self.engine.outcome().map(Into::into)
    }

    fn refused(&mut self) -> bool {
        self.engine.refused()
    }
}
