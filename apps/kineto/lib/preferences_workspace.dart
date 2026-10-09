import 'package:flutter/material.dart';
import 'package:wabisabi/wabisabi.dart';

import 'kineto_controls.dart';
import 'kineto_work.dart';
import 'native/kineto_engine.dart';
import 'screenplay_workspace.dart';

/// How many new choices make asking for suggestions worth it, as the engine
/// counts them.
const int choicesWorthAsking = 5;

class PreferencesWorkspaceScreen extends StatefulWidget {
  const PreferencesWorkspaceScreen({
    required this.work,
    required this.project,
    this.navigation,
    this.isDarkMode = true,
    this.onThemeToggle,
    super.key,
  });

  final KinetoProjectSession project;

  /// Switches to the other workspaces, shown above the preferences.
  final Widget? navigation;
  final bool isDarkMode;
  final VoidCallback? onThemeToggle;

  /// The project's requests, which outlive this page.
  final KinetoWork work;

  @override
  State<PreferencesWorkspaceScreen> createState() => _PreferencesWorkspaceScreenState();
}

class _PreferencesWorkspaceScreenState extends State<PreferencesWorkspaceScreen> {
  late final KinetoLanguageModel _model;
  late KinetoVoices _voices;
  int? _suggestCost;
  static const String _page = 'preferences';
  int _finished = 0;

  // The project's requests outlive this page: what is under way, and how
  // the last one asked for here ended, come from them.
  bool get _working => widget.work.busy;
  String get _workingLabel => widget.work.running?.label ?? 'Working';
  String? get _error => widget.work.problemOn(_page);
  VoidCallback? get _resubmit => widget.work.retryOn(_page);

  @override
  void initState() {
    super.initState();
    _finished = widget.work.finished;
    widget.work.addListener(_workChanged);
    _model = widget.project.languageModel;
    _read();
  }

  void _read() {
    _voices = widget.project.voices;
    _suggestCost = widget.project.documentCostMicros(KinetoDocumentKind.preferenceSuggestions);
  }

  void _workChanged() {
    if (!mounted) return;
    setState(() {
      if (widget.work.finished != _finished) {
        _finished = widget.work.finished;
        _read();
      }
    });
  }

  @override
  void dispose() {
    widget.work.removeListener(_workChanged);
    super.dispose();
  }

  Future<void> _run(Future<void> Function() action, String label, {Future<void> Function()? resubmit}) async {
    await widget.work.run(_page, label, action, retry: resubmit);
  }

  @override
  Widget build(BuildContext context) {
    final project = widget.project;
    return PreferencesWorkspaceView(
      voices: _voices,
      model: _model,
      suggestCostMicros: _suggestCost,
      working: _working,
      workingLabel: _workingLabel,
      error: _error,
      onResubmit: _resubmit,
      navigation: widget.navigation,
      isDarkMode: widget.isDarkMode,
      onThemeToggle: widget.onThemeToggle,
      onSave: (entries) => _run(() => project.savePreferences(_voices.preferencesRevision, entries), 'Saving'),
      onSuggest: () => _run(
        project.suggestPreferences,
        'Asking ${_model.kind.label} what your choices show',
        resubmit: () => project.suggestPreferences(resubmit: true),
      ),
      onAccept: (suggestion, value) => _run(() => project.acceptSuggestion(suggestion, value: value), 'Accepting'),
      onReject: (suggestion) => _run(() => project.rejectSuggestion(suggestion), 'Rejecting'),
    );
  }
}

class PreferencesWorkspaceView extends StatelessWidget {
  const PreferencesWorkspaceView({
    required this.voices,
    required this.model,
    required this.onSave,
    required this.onSuggest,
    required this.onAccept,
    required this.onReject,
    this.suggestCostMicros,
    this.working = false,
    this.workingLabel = 'Working',
    this.error,
    this.onResubmit,
    this.navigation,
    this.isDarkMode = true,
    this.onThemeToggle,
    super.key,
  });

  final KinetoVoices voices;
  final KinetoLanguageModel model;
  final int? suggestCostMicros;
  final bool working;
  final String workingLabel;
  final String? error;
  final VoidCallback? onResubmit;
  final Widget? navigation;
  final bool isDarkMode;
  final VoidCallback? onThemeToggle;

  /// Save the whole profile as the next revision.
  final ValueChanged<List<KinetoPreference>> onSave;
  final VoidCallback onSuggest;

  /// Accept a suggestion, as suggested (null) or in the director's words.
  final void Function(KinetoSuggestion suggestion, String? value) onAccept;
  final ValueChanged<KinetoSuggestion> onReject;

  @override
  Widget build(BuildContext context) => WabScaffold(
        appBar: WabAppBar(
          title: const Text('Kineto'),
          action: onThemeToggle == null
              ? null
              : KinetoThemeToggle(isDarkMode: isDarkMode, onToggle: onThemeToggle),
        ),
        body: SingleChildScrollView(
          padding: const EdgeInsets.all(24),
          child: Center(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 1040),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: <Widget>[
                  if (navigation != null) ...<Widget>[navigation!, const SizedBox(height: 20)],
                  if (error != null) ...<Widget>[
                    WabPanel(
                      title: 'That did not work',
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: <Widget>[
                          Text(error!),
                          if (onResubmit != null) ...<Widget>[
                            const SizedBox(height: 12),
                            KinetoButton(
                              key: const ValueKey<String>('resubmit-paid'),
                              onPressed: onResubmit,
                              child: const Text('Ask again'),
                            ),
                          ],
                        ],
                      ),
                    ),
                    const SizedBox(height: 20),
                  ],
                  // Keyed by revision: a saved profile replaces the editor.
                  PreferenceEditor(
                    key: ValueKey<int?>(voices.preferencesRevision),
                    preferences: voices.preferences,
                    revision: voices.preferencesRevision,
                    revisions: voices.preferenceRevisions,
                    planChanged: voices.planPreferencesChanged,
                    enabled: !working,
                    onSave: onSave,
                  ),
                  const SizedBox(height: 20),
                  _suggestionsPanel(),
                ],
              ),
            ),
          ),
        ),
      );

  Widget _suggestionsPanel() => WabPanel(
        title: 'Suggestions from your choices',
        trailing: voices.suggestionsReady
            ? const WabStatusBadge('Worth asking', kind: WabBadgeKind.primary)
            : null,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: <Widget>[
            Text(
              voices.unconsideredChoices == 0
                  ? 'No new choices since the last suggestions. Choose and reject candidates, and '
                      'the model compares what you chose with what you did not.'
                  : '${voices.unconsideredChoices} new choice${voices.unconsideredChoices == 1 ? '' : 's'}'
                      ' since the last suggestions; $choicesWorthAsking make it worth asking.',
            ),
            const SizedBox(height: 4),
            const Text('Nothing is taken in until you accept it.'),
            const SizedBox(height: 12),
            Align(
              alignment: Alignment.centerLeft,
              child: KinetoButton(
                key: const ValueKey<String>('suggest-preferences'),
                onPressed: !working && model.ready && voices.unconsideredChoices > 0 ? onSuggest : null,
                child: Text('Ask for suggestions · ${documentCostLabel(model, suggestCostMicros)}'),
              ),
            ),
            for (final suggestion in voices.suggestions) ...<Widget>[
              const SizedBox(height: 12),
              SuggestionCard(
                suggestion: suggestion,
                enabled: !working,
                onAccept: (value) => onAccept(suggestion, value),
                onReject: () => onReject(suggestion),
              ),
            ],
          ],
        ),
      );
}

/// The director's profile, edited entry by entry and saved whole.
class PreferenceEditor extends StatefulWidget {
  const PreferenceEditor({
    required this.preferences,
    required this.enabled,
    required this.onSave,
    this.revision,
    this.revisions = const <KinetoPreferenceRevision>[],
    this.planChanged = false,
    super.key,
  });

  final List<KinetoPreference> preferences;
  final int? revision;
  final List<KinetoPreferenceRevision> revisions;

  /// The shot plan in play was made with other preferences.
  final bool planChanged;
  final bool enabled;
  final ValueChanged<List<KinetoPreference>> onSave;

  @override
  State<PreferenceEditor> createState() => _PreferenceEditorState();
}

class _Draft {
  _Draft(this.preference) : value = TextEditingController(text: preference.value);

  KinetoPreference preference;
  final TextEditingController value;
}

class _PreferenceEditorState extends State<PreferenceEditor> {
  late final List<_Draft> _drafts = <_Draft>[for (final preference in widget.preferences) _Draft(preference)];
  String? _problem;

  @override
  void dispose() {
    for (final draft in _drafts) {
      draft.value.dispose();
    }
    super.dispose();
  }

  void _save() {
    if (_drafts.any((draft) => draft.value.text.trim().isEmpty)) {
      setState(() => _problem = 'Every preference needs words: what to prefer or avoid.');
      return;
    }
    setState(() => _problem = null);
    widget.onSave(<KinetoPreference>[
      for (final draft in _drafts) draft.preference.copyWith(value: draft.value.text.trim()),
    ]);
  }

  @override
  Widget build(BuildContext context) {
    final revision = widget.revision;
    return WabPanel(
      title: revision == null ? 'Your preferences' : 'Your preferences · revision $revision',
      trailing: widget.planChanged
          ? const WabStatusBadge('The shot plan was made with others', kind: WabBadgeKind.warning)
          : null,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          const Text(
            'What you prefer and avoid is said to the planner, the keyframes and the videos after the '
            'locked style. Changing it never makes work stale; work made with others says so.',
          ),
          if (widget.revisions.length > 1) ...<Widget>[
            const SizedBox(height: 8),
            Text(
              'Revisions: ${widget.revisions.map((revision) => '${revision.number} (${revision.origin.wireName})').join(', ')}',
            ),
          ],
          for (final (index, draft) in _drafts.indexed) ...<Widget>[
            const SizedBox(height: 12),
            KeyedSubtree(key: ObjectKey(draft), child: _entry(index, draft)),
          ],
          if (_problem != null) ...<Widget>[const SizedBox(height: 8), Text(_problem!)],
          const SizedBox(height: 12),
          Wrap(
            spacing: 12,
            children: <Widget>[
              KinetoButton(
                key: const ValueKey<String>('add-preference'),
                onPressed: widget.enabled
                    ? () => setState(
                          () => _drafts.add(
                            _Draft(
                              const KinetoPreference(
                                dimension: KinetoDimension.lighting,
                                stance: KinetoStance.prefer,
                                value: '',
                              ),
                            ),
                          ),
                        )
                    : null,
                child: const Text('Add a preference'),
              ),
              KinetoButton(
                key: const ValueKey<String>('save-preferences'),
                onPressed: widget.enabled ? _save : null,
                child: const Text('Save'),
              ),
            ],
          ),
        ],
      ),
    );
  }

  Widget _entry(int index, _Draft draft) {
    final preference = draft.preference;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: <Widget>[
        Wrap(
          spacing: 12,
          crossAxisAlignment: WrapCrossAlignment.center,
          children: <Widget>[
            DropdownButton<KinetoStance>(
              key: ValueKey<String>('preference-$index-stance'),
              value: preference.stance,
              items: <DropdownMenuItem<KinetoStance>>[
                for (final stance in KinetoStance.values)
                  DropdownMenuItem<KinetoStance>(value: stance, child: Text(stance.label)),
              ],
              onChanged: widget.enabled
                  ? (stance) => setState(() => draft.preference = preference.copyWith(stance: stance))
                  : null,
            ),
            DropdownButton<KinetoDimension>(
              key: ValueKey<String>('preference-$index-dimension'),
              value: preference.dimension,
              items: <DropdownMenuItem<KinetoDimension>>[
                for (final dimension in KinetoDimension.values)
                  DropdownMenuItem<KinetoDimension>(value: dimension, child: Text(dimension.label)),
              ],
              onChanged: widget.enabled
                  ? (dimension) => setState(() => draft.preference = preference.copyWith(dimension: dimension))
                  : null,
            ),
            SizedBox(
              width: 320,
              child: TextField(
                key: ValueKey<String>('preference-$index-value'),
                controller: draft.value,
                enabled: widget.enabled,
                decoration: wabInputDecoration(hintText: 'In your words, such as “slow dolly”'),
              ),
            ),
            TextButton(
              key: ValueKey<String>('preference-$index-remove'),
              onPressed: widget.enabled ? () => setState(() => _drafts.removeAt(index).value.dispose()) : null,
              child: const Text('Remove'),
            ),
          ],
        ),
        if (preference.fromSuggestion)
          Text('From a suggestion: ${preference.reason}', key: ValueKey<String>('preference-$index-reason')),
      ],
    );
  }
}

/// A suggestion to accept, in its words or the director's, or reject.
class SuggestionCard extends StatefulWidget {
  const SuggestionCard({
    required this.suggestion,
    required this.enabled,
    required this.onAccept,
    required this.onReject,
    super.key,
  });

  final KinetoSuggestion suggestion;
  final bool enabled;
  final ValueChanged<String?> onAccept;
  final VoidCallback onReject;

  @override
  State<SuggestionCard> createState() => _SuggestionCardState();
}

class _SuggestionCardState extends State<SuggestionCard> {
  late final TextEditingController _words = TextEditingController(text: widget.suggestion.value);

  @override
  void dispose() {
    _words.dispose();
    super.dispose();
  }

  String get _key => '${widget.suggestion.batch}-${widget.suggestion.index}';

  @override
  Widget build(BuildContext context) {
    final suggestion = widget.suggestion;
    return WabPanel(
      title: '${suggestion.stance.label} · ${suggestion.dimension.label.toLowerCase()} · ${suggestion.value}',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          Text(suggestion.reason),
          const SizedBox(height: 4),
          Text(
            'Seen in ${suggestion.evidence.join(', ')}'
            '${suggestion.comparedImages ? '' : ' (compared by their prompts and notes, not the images)'}',
          ),
          const SizedBox(height: 8),
          TextField(
            key: ValueKey<String>('suggestion-$_key-words'),
            controller: _words,
            enabled: widget.enabled,
            onChanged: (_) => setState(() {}),
            decoration: wabInputDecoration(hintText: 'In your words'),
          ),
          const SizedBox(height: 8),
          Wrap(
            spacing: 12,
            children: <Widget>[
              KinetoButton(
                key: ValueKey<String>('suggestion-$_key-accept'),
                onPressed: widget.enabled && _words.text.trim().isNotEmpty
                    ? () => widget.onAccept(_words.text.trim() == suggestion.value ? null : _words.text.trim())
                    : null,
                child: const Text('Accept'),
              ),
              KinetoButton(
                key: ValueKey<String>('suggestion-$_key-reject'),
                onPressed: widget.enabled ? widget.onReject : null,
                child: const Text('Reject'),
              ),
            ],
          ),
        ],
      ),
    );
  }
}
