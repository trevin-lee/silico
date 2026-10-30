//! The commands that run in a session, by name ([`SessionCommand`]): what a command's stop
//! is named by, what `status` says is running, and what `cancel` stops; and those of them
//! that read a model, the current one or the published one ([`ReadCommand`]), which
//! `status` lists among its reads when they read the published one.

use super::call::Command;

/// A command that runs in a session (docs/design/m4.1-command-api.md, section 7): every
/// command but those answered at once, never running (`version`, `status`, `cancel`,
/// `read_logs`, and `schema`), and `open`, which runs before its session is there to name
/// it. Its name is the command's ([`command`](Self::command)), as requests and tools give it.
///
/// `undo` and `redo` (M5.2) run in a session as every other command does, so `status` says
/// they're running and `cancel` stops them, but no transport names them yet: they're in no
/// list a client reads ([`ALL`](Self::ALL), [`Command`]), whose change, with MCP's tools and
/// the protected contracts, comes with M5.2's wire step (docs/design/m5.0-app.md, section
/// 7).
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
    SuppressFeature,
    Timeline,
    Undo,
    Redo,
}

impl SessionCommand {
    /// Every command that runs in a session that a transport names, in the design's order
    /// (M4.1's, then M5.0's): all but `undo` and `redo`, which none names yet.
    pub const ALL: [SessionCommand; 23] = [
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
        SessionCommand::SuppressFeature,
        SessionCommand::Timeline,
    ];

    /// The command it is, of every command a transport names ([`Command`]): none for `undo`
    /// and `redo`, which none names yet.
    pub const fn command(self) -> Option<Command> {
        match self.named() {
            Ok(command) => Some(command),
            Err(_) => None,
        }
    }

    /// The command's name, as requests, `status`, and `cancel` give it.
    pub const fn as_str(self) -> &'static str {
        match self.named() {
            Ok(command) => command.as_str(),
            Err(name) => name,
        }
    }

    /// The command it is, of every command a transport names; or, for one none names yet,
    /// its name.
    const fn named(self) -> Result<Command, &'static str> {
        Ok(match self {
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
            SessionCommand::SuppressFeature => Command::SuppressFeature,
            SessionCommand::Timeline => Command::Timeline,
            SessionCommand::Undo => return Err("undo"),
            SessionCommand::Redo => return Err("redo"),
        })
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
