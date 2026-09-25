use codex_protocol::protocol::Event;

/// Extension warning with an explicit thread target and optional turn correlation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionWarning {
    /// Stable host-owned thread identifier used for delivery.
    pub thread_id: String,
    /// Stable host-owned turn identifier when the warning arose in a turn callback.
    pub turn_id: Option<String>,
    /// Concise warning message for the user.
    pub message: String,
}

/// Result of the optional Jev skill-selection pass for one turn.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtensionSkillSuggestion {
    /// Stable host-owned thread identifier used for delivery.
    pub thread_id: String,
    /// Stable host-owned turn identifier used for correlation.
    pub turn_id: String,
    /// Whether Jev found any sufficiently relevant skills or could not run.
    pub status: ExtensionSkillSuggestionStatus,
    /// Skills above the configured relevance threshold, ordered by fit probability.
    pub suggestions: Vec<ExtensionSkillSuggestionItem>,
}

/// User-visible state for a Jev skill-selection request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionSkillSuggestionStatus {
    Suggested,
    NoMatch,
    Unavailable,
}

/// One high-confidence skill recommendation from Jev.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtensionSkillSuggestionItem {
    pub name: String,
    pub fit_probability: f64,
}

/// Host-provided fire-and-forget sink for extension-generated events.
///
/// Extensions construct protocol events with the correlation id appropriate for
/// the callback they are handling, then leave persistence, ordering, transport
/// fanout, and logging decisions to the host.
pub trait ExtensionEventSink: Send + Sync {
    /// Queue one protocol event for host-owned delivery.
    fn emit(&self, event: Event);

    /// Queue one warning for host-owned delivery.
    ///
    /// Implementations must use [`ExtensionWarning::thread_id`] for routing. The optional
    /// [`ExtensionWarning::turn_id`] is correlation metadata and does not identify a thread.
    fn emit_warning(&self, warning: ExtensionWarning);

    /// Queue Jev's bounded skill-suggestion status for the current turn.
    fn emit_skill_suggestion(&self, _suggestion: ExtensionSkillSuggestion) {}
}

/// Event sink used when the host does not expose extension event emission.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopExtensionEventSink;

impl ExtensionEventSink for NoopExtensionEventSink {
    fn emit(&self, _event: Event) {}

    fn emit_warning(&self, _warning: ExtensionWarning) {}
}
