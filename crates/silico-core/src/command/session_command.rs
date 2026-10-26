//! The commands that run in a session, by name ([`SessionCommand`]): what a command's stop
//! is named by, what `status` says is running, and what `cancel` stops; and those of them
//! that may read the published model ([`PublishedRead`]), which `status` lists among its
//! reads.

/// A command that runs in a session (docs/design/m4.1-command-api.md, section 7): every
/// command but those answered at once, never running (`version`, `status`, `cancel`, and
/// `schema`), and `open`, which runs before its session is there to name it. Its name is the
/// one requests and tools give it ([`as_str`](Self::as_str)).
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
    Elements,
    Query,
    MassProperties,
    Distance,
    Angle,
    Render,
}

impl SessionCommand {
    /// Every command that runs in a session, in the design's order.
    pub const ALL: [SessionCommand; 18] = [
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
        SessionCommand::Elements,
        SessionCommand::Query,
        SessionCommand::MassProperties,
        SessionCommand::Distance,
        SessionCommand::Angle,
        SessionCommand::Render,
    ];

    /// The command's name, as requests, `status`, and `cancel` give it.
    pub const fn as_str(self) -> &'static str {
        match self {
            SessionCommand::Close => "close",
            SessionCommand::Begin => "begin",
            SessionCommand::Commit => "commit",
            SessionCommand::Rollback => "rollback",
            SessionCommand::Format => "format",
            SessionCommand::AddFeature => "add_feature",
            SessionCommand::SetField => "set_field",
            SessionCommand::RemoveFeature => "remove_feature",
            SessionCommand::ReplaceText => "replace_text",
            SessionCommand::Text => "text",
            SessionCommand::Outline => "outline",
            SessionCommand::Check => "check",
            SessionCommand::Elements => "elements",
            SessionCommand::Query => "query",
            SessionCommand::MassProperties => "mass_properties",
            SessionCommand::Distance => "distance",
            SessionCommand::Angle => "angle",
            SessionCommand::Render => "render",
        }
    }
}

impl std::fmt::Display for SessionCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A command that may run as a read of the published model
/// ([`Sessions::read_model`](super::Sessions::read_model)'s): what `status` lists among its
/// reads, and `cancel` stops as the session command it is ([`SessionCommand::from`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PublishedRead {
    Check,
    Elements,
    Query,
    MassProperties,
    Distance,
    Angle,
    Render,
}

impl PublishedRead {
    /// Every command that may read the published model, in the design's order (that of
    /// [`SessionCommand::ALL`]).
    pub const ALL: [PublishedRead; 7] = [
        PublishedRead::Check,
        PublishedRead::Elements,
        PublishedRead::Query,
        PublishedRead::MassProperties,
        PublishedRead::Distance,
        PublishedRead::Angle,
        PublishedRead::Render,
    ];

    /// The session command it is.
    pub const fn command(self) -> SessionCommand {
        match self {
            PublishedRead::Check => SessionCommand::Check,
            PublishedRead::Elements => SessionCommand::Elements,
            PublishedRead::Query => SessionCommand::Query,
            PublishedRead::MassProperties => SessionCommand::MassProperties,
            PublishedRead::Distance => SessionCommand::Distance,
            PublishedRead::Angle => SessionCommand::Angle,
            PublishedRead::Render => SessionCommand::Render,
        }
    }

    /// The command's name, as requests, `status`, and `cancel` give it.
    pub const fn as_str(self) -> &'static str {
        self.command().as_str()
    }
}

impl From<PublishedRead> for SessionCommand {
    fn from(read: PublishedRead) -> Self {
        read.command()
    }
}

impl std::fmt::Display for PublishedRead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
