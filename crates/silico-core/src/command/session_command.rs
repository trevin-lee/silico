//! The commands that run in a session, by name ([`SessionCommand`]): what a command's stop
//! is named by, what `status` says is running, and what `cancel` stops; and those of them
//! that read a model, the current one or the published one ([`ReadCommand`]), which
//! `status` lists among its reads when they read the published one.

use super::call::Command;

/// A command that runs in a session (docs/design/m4.1-command-api.md, section 7): every
/// command but those answered at once, never running (`version`, `status`, `cancel`,
/// `read_logs`, and `schema`), and `open`, which runs before its session is there to name
/// it. Its name is the command's ([`command`](Self::command)), as requests and tools give it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SessionCommand {
    Close,
    Begin,
    Commit,
    Rollback,
    Format,
    AddFeature,
    SetField,
    RemoveFeature,
    ReplaceText,
    Text,
    Outline,
    Check,
    CacheClear,
    Elements,
    Query,
    MassProperties,
    Distance,
    Angle,
    Render,
    Export,
    MoveFeature,
    Timeline,
    Undo,
    Redo,
}

impl SessionCommand {
    /// Every command that runs in a session, in the design's order (M4.1's, then M5.0's).
    pub const ALL: [SessionCommand; 24] = [
        SessionCommand::Close,
        SessionCommand::Begin,
        SessionCommand::Commit,
        SessionCommand::Rollback,
        SessionCommand::Format,
        SessionCommand::AddFeature,
        SessionCommand::SetField,
        SessionCommand::RemoveFeature,
        SessionCommand::ReplaceText,
        SessionCommand::Text,
        SessionCommand::Outline,
        SessionCommand::Check,
        SessionCommand::CacheClear,
        SessionCommand::Elements,
        SessionCommand::Query,
        SessionCommand::MassProperties,
        SessionCommand::Distance,
        SessionCommand::Angle,
        SessionCommand::Render,
        SessionCommand::Export,
        SessionCommand::MoveFeature,
        SessionCommand::Timeline,
        SessionCommand::Undo,
        SessionCommand::Redo,
    ];

    /// The command it is, of every command ([`Command`]).
    pub const fn command(self) -> Command {
        match self {
            SessionCommand::Close => Command::Close,
            SessionCommand::Begin => Command::Begin,
            SessionCommand::Commit => Command::Commit,
            SessionCommand::Rollback => Command::Rollback,
            SessionCommand::Format => Command::Format,
            SessionCommand::AddFeature => Command::AddFeature,
            SessionCommand::SetField => Command::SetField,
            SessionCommand::RemoveFeature => Command::RemoveFeature,
            SessionCommand::ReplaceText => Command::ReplaceText,
            SessionCommand::Text => Command::Text,
            SessionCommand::Outline => Command::Outline,
            SessionCommand::Check => Command::Check,
            SessionCommand::CacheClear => Command::CacheClear,
            SessionCommand::Elements => Command::Elements,
            SessionCommand::Query => Command::Query,
            SessionCommand::MassProperties => Command::MassProperties,
            SessionCommand::Distance => Command::Distance,
            SessionCommand::Angle => Command::Angle,
            SessionCommand::Render => Command::Render,
            SessionCommand::Export => Command::Export,
            SessionCommand::MoveFeature => Command::MoveFeature,
            SessionCommand::Timeline => Command::Timeline,
            SessionCommand::Undo => Command::Undo,
            SessionCommand::Redo => Command::Redo,
        }
    }

    /// The command's name, as requests, `status`, and `cancel` give it.
    pub const fn as_str(self) -> &'static str {
        self.command().as_str()
    }
}

/// What `status` says is running on a session's thread (docs/design/m5.0-app.md, A3): one of
/// its queued commands, or the session's own reload of its file, found changed on disk with
/// no transaction open, which no client sent and none can cancel (`cancel`'s `command` is a
/// [`SessionCommand`], which this isn't).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunningCommand {
    /// A command a client sent.
    Command(SessionCommand),
    /// The session's own reload.
    Reload,
}

impl RunningCommand {
    /// Its name, as `status` and `read_logs`' spans give it: the command's, or `reload`.
    pub const fn as_str(self) -> &'static str {
        match self {
            RunningCommand::Command(command) => command.as_str(),
            RunningCommand::Reload => "reload",
        }
    }

    /// The command a client sent, if it's one: what `cancel` can stop.
    pub const fn command(self) -> Option<SessionCommand> {
        match self {
            RunningCommand::Command(command) => Some(command),
            RunningCommand::Reload => None,
        }
    }
}

impl std::fmt::Display for SessionCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A command that reads a model, the current one or the published one
/// ([`Sessions::read_model`](super::Sessions::read_model)'s): as a read of the published
/// model, what `status` lists among its reads, and `cancel` stops as the session command it
/// is ([`SessionCommand::from`]). A command that reads the current model only (`export`)
/// isn't one: it's queued as its session command
/// (`Sessions::read_current`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReadCommand {
    Check,
    Elements,
    Query,
    MassProperties,
    Distance,
    Angle,
    Render,
    Timeline,
}

impl ReadCommand {
    /// Every command that reads a model, in the design's order (that of
    /// [`SessionCommand::ALL`]).
    pub const ALL: [ReadCommand; 8] = [
        ReadCommand::Check,
        ReadCommand::Elements,
        ReadCommand::Query,
        ReadCommand::MassProperties,
        ReadCommand::Distance,
        ReadCommand::Angle,
        ReadCommand::Render,
        ReadCommand::Timeline,
    ];

    /// The session command it is.
    pub const fn command(self) -> SessionCommand {
        match self {
            ReadCommand::Check => SessionCommand::Check,
            ReadCommand::Elements => SessionCommand::Elements,
            ReadCommand::Query => SessionCommand::Query,
            ReadCommand::MassProperties => SessionCommand::MassProperties,
            ReadCommand::Distance => SessionCommand::Distance,
            ReadCommand::Angle => SessionCommand::Angle,
            ReadCommand::Render => SessionCommand::Render,
            ReadCommand::Timeline => SessionCommand::Timeline,
        }
    }

    /// The command's name, as requests, `status`, and `cancel` give it.
    pub const fn as_str(self) -> &'static str {
        self.command().as_str()
    }
}

impl From<ReadCommand> for SessionCommand {
    fn from(read: ReadCommand) -> Self {
        read.command()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_runs_is_a_command_cancel_can_name_or_the_reload_it_cant() {
        // `cancel` stops only what a client sent: each command is its own, the session's
        // reload none.
        for command in SessionCommand::ALL {
            let running = RunningCommand::Command(command);
            assert_eq!(running.command(), Some(command));
            assert_eq!(running.as_str(), command.as_str());
        }
        assert_eq!(RunningCommand::Reload.command(), None);
        assert_eq!(RunningCommand::Reload.as_str(), "reload");
    }
}
