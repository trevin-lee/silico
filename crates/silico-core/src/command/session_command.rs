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
    SuppressFeature,
    Timeline,
    Sketch,
    SketchAdd,
    SketchRemove,
    SketchSplit,
    SketchTrim,
    SketchUnjoin,
    SketchMerge,
    Undo,
    Redo,
}

impl SessionCommand {
    /// Every command that runs in a session, in the design's order (M4.1's, then M5.0's).
    pub const ALL: [SessionCommand; 32] = [
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
        SessionCommand::Sketch,
        SessionCommand::SketchAdd,
        SessionCommand::SketchRemove,
        SessionCommand::SketchSplit,
        SessionCommand::SketchTrim,
        SessionCommand::SketchUnjoin,
        SessionCommand::SketchMerge,
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
            SessionCommand::SuppressFeature => Command::SuppressFeature,
            SessionCommand::Timeline => Command::Timeline,
            SessionCommand::Sketch => Command::Sketch,
            SessionCommand::SketchAdd => Command::SketchAdd,
            SessionCommand::SketchRemove => Command::SketchRemove,
            SessionCommand::SketchSplit => Command::SketchSplit,
            SessionCommand::SketchTrim => Command::SketchTrim,
            SessionCommand::SketchUnjoin => Command::SketchUnjoin,
            SessionCommand::SketchMerge => Command::SketchMerge,
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

/// What `cancel` names (decision 34, docs/design/m4.1-command-api.md, section 6): whatever
/// `status` reports as running, by the key it reports it under. A command a client sent, by
/// its name (`status.running`, `status.reads`), or the session's rebuild running, `rebuild`
/// (`status.rebuild.state` is `running`). A rebuild and the one command waiting on it stop
/// together, whichever is named. The session's own reload is neither: nothing cancels it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Cancellable {
    /// A command a client sent.
    Command(SessionCommand),
    /// The session's rebuild running.
    Rebuild,
}

impl Cancellable {
    /// Everything `cancel` names, in the design's order: every command that runs in a
    /// session ([`SessionCommand::ALL`]), then the rebuild.
    pub const ALL: [Cancellable; SessionCommand::ALL.len() + 1] = {
        let mut all = [Cancellable::Rebuild; SessionCommand::ALL.len() + 1];
        let mut at = 0;
        while at < SessionCommand::ALL.len() {
            all[at] = Cancellable::Command(SessionCommand::ALL[at]);
            at += 1;
        }
        all
    };

    /// Its name, as `cancel`'s request and result give it: the command's, or `rebuild`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Cancellable::Command(command) => command.as_str(),
            Cancellable::Rebuild => "rebuild",
        }
    }

    /// The command a client sent, if it's one.
    pub const fn command(self) -> Option<SessionCommand> {
        match self {
            Cancellable::Command(command) => Some(command),
            Cancellable::Rebuild => None,
        }
    }
}

impl From<SessionCommand> for Cancellable {
    fn from(command: SessionCommand) -> Self {
        Cancellable::Command(command)
    }
}

impl std::fmt::Display for Cancellable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
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
    Sketch,
    Timeline,
}

impl ReadCommand {
    /// Every command that reads a model, in the design's order (that of
    /// [`SessionCommand::ALL`]).
    pub const ALL: [ReadCommand; 9] = [
        ReadCommand::Check,
        ReadCommand::Elements,
        ReadCommand::Query,
        ReadCommand::MassProperties,
        ReadCommand::Distance,
        ReadCommand::Angle,
        ReadCommand::Render,
        ReadCommand::Timeline,
        ReadCommand::Sketch,
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
            ReadCommand::Sketch => SessionCommand::Sketch,
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

    #[test]
    fn cancel_names_each_command_in_the_designs_order_then_the_rebuild() {
        // Decision 34 as amended: whatever status reports running, by its key. Every
        // command, as itself, then `rebuild`, last, which no command is named.
        assert_eq!(Cancellable::ALL.len(), SessionCommand::ALL.len() + 1);
        for (at, command) in SessionCommand::ALL.into_iter().enumerate() {
            assert_eq!(Cancellable::ALL[at], Cancellable::Command(command));
            assert_eq!(Cancellable::from(command).as_str(), command.as_str());
            assert_eq!(Cancellable::from(command).command(), Some(command));
        }
        assert_eq!(Cancellable::ALL.last(), Some(&Cancellable::Rebuild));
        assert_eq!(Cancellable::Rebuild.as_str(), "rebuild");
        assert_eq!(Cancellable::Rebuild.command(), None);
        assert!(
            SessionCommand::ALL
                .iter()
                .all(|command| command.as_str() != "rebuild")
        );
    }
}
