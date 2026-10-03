import 'package:flutter/material.dart';
import 'package:wabisabi/wabisabi.dart';

import 'native/kineto_engine.dart';

/// What the director saved from the scene editor.
final class SceneDraft {
  const SceneDraft({
    required this.sceneId,
    required this.heading,
    required this.time,
    required this.dramaticGoal,
    required this.blocks,
  });

  final String sceneId;
  final String heading;
  final String time;
  final String dramaticGoal;
  final List<KinetoScreenplayBlock> blocks;
}

/// [micros] USD as dollars, for example `$0.0035`.
String formatDollars(int micros) {
  final dollars = micros ~/ 1000000;
  final fraction = (micros % 1000000).toString().padLeft(6, '0').replaceFirst(RegExp(r'0+$'), '');
  return fraction.isEmpty ? '\$$dollars' : '\$$dollars.$fraction';
}

/// What asking [model] for a document costs, in words.
String documentCostLabel(KinetoLanguageModel model, int? micros) {
  if (!model.ready) return 'no model configured';
  if (model.isAgent) return 'cost unknown: billed by the Kimi Code subscription';
  if (micros == null) return 'cost unknown';
  if (micros == 0) return 'free';
  return 'at most ${formatDollars(micros)}';
}

/// Why documents cannot be asked for, when they cannot.
String? languageModelProblem(KinetoLanguageModel model) {
  if (model.ready) return null;
  return switch (model.kind) {
    KinetoLanguageModelKind.none =>
      'No language model is configured. Set KINETO_LLM_PROVIDER to kimi-code, kimi or local '
          'and reopen Kineto.',
    KinetoLanguageModelKind.kimiCode =>
      'Kimi Code is selected but its command-line tool was not found; install it or set '
          'KINETO_KIMI_CODE_BIN, then reopen Kineto.',
    KinetoLanguageModelKind.kimi =>
      'Kimi API is selected but has no key; set KINETO_KIMI_API_KEY and reopen Kineto.',
    KinetoLanguageModelKind.local =>
      'A local model is selected; set KINETO_LOCAL_LLM_URL and KINETO_LOCAL_LLM_MODEL and '
          'reopen Kineto.',
  };
}

class ScreenplayWorkspaceScreen extends StatefulWidget {
  const ScreenplayWorkspaceScreen({
    required this.project,
    this.navigation,
    this.isDarkMode = true,
    this.onThemeToggle,
    super.key,
  });

  final KinetoProjectSession project;

  /// Switches to the other workspaces, shown above the story.
  final Widget? navigation;
  final bool isDarkMode;
  final VoidCallback? onThemeToggle;

  @override
  State<ScreenplayWorkspaceScreen> createState() => _ScreenplayWorkspaceScreenState();
}

class _ScreenplayWorkspaceScreenState extends State<ScreenplayWorkspaceScreen> {
  late final KinetoLanguageModel _model;
  late KinetoDocuments _documents;
  int? _analysisCost;
  int? _screenplayCost;
  bool _working = false;
  String _workingLabel = 'Working';
  String? _error;

  /// Asks again for what the last command asked for although its paid
  /// request may already have been charged; offered only after that refusal.
  VoidCallback? _resubmit;
  String? _editingScene;

  @override
  void initState() {
    super.initState();
    _model = widget.project.languageModel;
    _read();
  }

  void _read() {
    _documents = widget.project.documents;
    _analysisCost = widget.project.documentCostMicros(KinetoDocumentKind.storyAnalysis);
    _screenplayCost = widget.project.documentCostMicros(KinetoDocumentKind.screenplay);
  }

  // Documents are written on the project's worker; this isolate only waits.
  // One command at a time keeps the page honest about what it shows.
  //
  // [resubmit] asks for the same document at the risk of paying twice; it is
  // offered when [action] is refused because its last paid request is
  // unresolved.
  Future<void> _run(
    Future<void> Function() action,
    String label, {
    VoidCallback? onDone,
    Future<void> Function()? resubmit,
  }) async {
    if (_working) return;
    setState(() {
      _working = true;
      _workingLabel = label;
      _error = null;
      _resubmit = null;
    });
    String? error;
    VoidCallback? offer;
    try {
      await action();
      onDone?.call();
    } on KinetoProjectException catch (failure) {
      error = failure.message;
      if (failure.code == KinetoProjectException.paidCallUnresolved && resubmit != null) {
        offer = () => _run(resubmit, label, onDone: onDone);
      }
    } on Object catch (failure) {
      error = failure.toString();
    }
    if (!mounted) return;
    setState(() {
      _working = false;
      _read();
      _error = error;
      _resubmit = offer;
    });
  }

  @override
  Widget build(BuildContext context) {
    final asking = 'with ${_model.kind.label}';
    return ScreenplayWorkspaceView(
      documents: _documents,
      model: _model,
      analysisCostMicros: _analysisCost,
      screenplayCostMicros: _screenplayCost,
      working: _working,
      workingLabel: _workingLabel,
      error: _error,
      onResubmit: _resubmit,
      editingSceneId: _editingScene,
      navigation: widget.navigation,
      isDarkMode: widget.isDarkMode,
      onThemeToggle: widget.onThemeToggle,
      onImport: (text) => _run(() => widget.project.importSource(text), 'Importing'),
      onAnalyze: () => _run(
        widget.project.analyzeStory,
        'Analysing the story $asking',
        resubmit: () => widget.project.analyzeStory(resubmit: true),
      ),
      onWrite: () => _run(
        widget.project.writeScreenplay,
        'Writing the screenplay $asking',
        resubmit: () => widget.project.writeScreenplay(resubmit: true),
      ),
      onRewrite: (sceneId, direction) => _run(
        () => widget.project.rewriteScene(sceneId, direction: direction),
        'Rewriting the scene $asking',
        resubmit: () => widget.project.rewriteScene(sceneId, direction: direction, resubmit: true),
      ),
      onEdit: (sceneId) => setState(() {
        _editingScene = sceneId;
        _error = null;
        _resubmit = null;
      }),
      onSave: (draft) => _run(
        () => widget.project.saveScene(
          basedOn: _documents.screenplay!.revision,
          sceneId: draft.sceneId,
          heading: draft.heading,
          time: draft.time,
          dramaticGoal: draft.dramaticGoal,
          blocks: draft.blocks,
        ),
        'Saving',
        onDone: () => _editingScene = null,
      ),
      onLock: (revision) => _run(() => widget.project.lockScreenplay(revision), 'Locking'),
    );
  }
}

class ScreenplayWorkspaceView extends StatelessWidget {
  const ScreenplayWorkspaceView({
    required this.documents,
    required this.model,
    required this.onImport,
    required this.onAnalyze,
    required this.onWrite,
    required this.onRewrite,
    required this.onEdit,
    required this.onSave,
    required this.onLock,
    this.analysisCostMicros,
    this.screenplayCostMicros,
    this.working = false,
    this.workingLabel = 'Working',
    this.error,
    this.onResubmit,
    this.editingSceneId,
    this.navigation,
    this.isDarkMode = true,
    this.onThemeToggle,
    super.key,
  });

  final KinetoDocuments documents;
  final KinetoLanguageModel model;
  final int? analysisCostMicros;
  final int? screenplayCostMicros;

  /// A command is running on the project's worker; changes wait for it.
  final bool working;
  final String workingLabel;
  final String? error;

  /// Asks again at the risk of paying twice; set only when [error] says the
  /// last paid request may have been charged without its answer being kept.
  final VoidCallback? onResubmit;

  /// The scene being edited, if any.
  final String? editingSceneId;
  final Widget? navigation;
  final bool isDarkMode;
  final VoidCallback? onThemeToggle;

  final ValueChanged<String> onImport;
  final VoidCallback onAnalyze;
  final VoidCallback onWrite;
  final void Function(String sceneId, String? direction) onRewrite;

  /// Start editing a scene, or stop with null.
  final ValueChanged<String?> onEdit;
  final ValueChanged<SceneDraft> onSave;
  final ValueChanged<int> onLock;

  bool get _idle => !working && editingSceneId == null;

  @override
  Widget build(BuildContext context) {
    final screenplay = documents.screenplay;
    final problem = languageModelProblem(model);
    return WabScaffold(
      appBar: WabAppBar(
        title: const Text('Kineto'),
        action: onThemeToggle == null
            ? null
            : WabToggleButton(
                text: const Text('Dark mode'),
                isOn: isDarkMode,
                pair: WabTogglePair.sealZhuwen,
                callback: onThemeToggle,
              ),
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
                if (working) ...<Widget>[
                  Align(
                    alignment: Alignment.centerLeft,
                    child: WabStatusBadge(workingLabel, kind: WabBadgeKind.progress),
                  ),
                  const SizedBox(height: 12),
                ],
                if (error != null) ...<Widget>[
                  WabPanel(
                    title: 'That did not work',
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: <Widget>[
                        Text(error!),
                        if (onResubmit != null) ...<Widget>[
                          const SizedBox(height: 12),
                          const Text(
                            'The model may have answered the last request, and charged for it, '
                            'without the answer reaching Kineto. Check your account before asking '
                            'again.',
                          ),
                          const SizedBox(height: 12),
                          WabButton(
                            key: const ValueKey<String>('resubmit-document'),
                            kind: WabMaterialKind.zhuwen,
                            onPressed: working ? null : onResubmit,
                            child: const Text('Ask again · may pay twice'),
                          ),
                        ],
                      ],
                    ),
                  ),
                  const SizedBox(height: 20),
                ],
                if (documents.damaged) ...<Widget>[
                  const WabPanel(
                    title: 'Documents cannot be read',
                    child: Text(
                      "A stored document no longer matches its record. Nothing was changed; restore "
                      "the project's files from a backup or version control.",
                    ),
                  ),
                  const SizedBox(height: 20),
                ],
                _story(context, problem),
                const SizedBox(height: 20),
                _screenplayPanel(context, screenplay),
                if (screenplay != null)
                  for (var index = 0; index < screenplay.scenes.length; index++) ...<Widget>[
                    const SizedBox(height: 20),
                    if (screenplay.scenes[index].id == editingSceneId)
                      SceneEditor(
                        key: ValueKey<String>('editor-${screenplay.revision}-${screenplay.scenes[index].id}'),
                        number: index + 1,
                        scene: screenplay.scenes[index],
                        cast: screenplay.cast,
                        working: working,
                        onCancel: () => onEdit(null),
                        onSave: onSave,
                      )
                    else
                      _scene(context, screenplay, index),
                  ],
                if (documents.revisions.isNotEmpty) ...<Widget>[
                  const SizedBox(height: 20),
                  _history(),
                ],
              ],
            ),
          ),
        ),
      ),
    );
  }

  Widget _story(BuildContext context, String? problem) {
    final analysis = documents.analysis;
    return WabPanel(
      title: 'Story',
      trailing: documents.sourceRevision == null ? null : Text('Source revision ${documents.sourceRevision}'),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          if (analysis == null)
            const Text('Not analysed yet. The analysis finds the characters, locations and scenes the screenplay is written from.')
          else ...<Widget>[
            Row(
              children: <Widget>[
                Text('Analysis ${analysis.revision}', style: Theme.of(context).textTheme.titleMedium),
                const SizedBox(width: 12),
                if (documents.analysisStale)
                  const WabStatusBadge('Source changed since', kind: WabBadgeKind.warning),
              ],
            ),
            const SizedBox(height: 8),
            Text(analysis.logline),
            const SizedBox(height: 8),
            Text('Characters: ${analysis.characterNames.join(', ')}'),
          ],
          const SizedBox(height: 16),
          if (problem != null) ...<Widget>[Text(problem), const SizedBox(height: 12)],
          Wrap(
            spacing: 12,
            runSpacing: 12,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: <Widget>[
              WabButton(
                kind: WabMaterialKind.zhuwen,
                onPressed: _idle ? () => _importDialog(context) : null,
                child: const Text('Import text'),
              ),
              WabButton(
                key: const ValueKey<String>('analyse'),
                kind: WabMaterialKind.seal,
                onPressed: _idle && model.ready ? onAnalyze : null,
                child: Text(analysis == null ? 'Analyse story' : 'Analyse again'),
              ),
              Text('${model.kind.label} · ${documentCostLabel(model, analysisCostMicros)}'),
            ],
          ),
        ],
      ),
    );
  }

  Widget _screenplayPanel(BuildContext context, KinetoScreenplay? screenplay) {
    final (status, kind) = screenplay == null
        ? ('None yet', WabBadgeKind.neutral)
        : documents.screenplayLocked
            ? ('Locked', WabBadgeKind.done)
            : documents.screenplayStale
                ? ('Analysis changed since', WabBadgeKind.warning)
                : ('Draft', WabBadgeKind.progress);
    final canWrite = _idle && model.ready && documents.analysis != null;
    return WabPanel(
      title: screenplay == null ? 'Screenplay' : 'Screenplay · revision ${screenplay.revision}',
      trailing: WabStatusBadge(status, kind: kind),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          if (screenplay != null) ...<Widget>[
            Text(screenplay.title, style: Theme.of(context).textTheme.titleLarge),
            const SizedBox(height: 8),
            Text(
              documents.screenplayLocked
                  ? 'Production follows this revision. Editing makes a new revision; the lock stays here until you lock another.'
                  : documents.lockedRevision == null
                      ? 'Review the scenes, edit or rewrite them, then lock the revision production should follow.'
                      : 'Revision ${documents.lockedRevision} is locked; this newer revision is a draft until you lock it.',
            ),
            const SizedBox(height: 16),
          ] else if (documents.analysis == null)
            const Padding(
              padding: EdgeInsets.only(bottom: 16),
              child: Text('Analyse the story first; the screenplay is written from the analysis.'),
            ),
          Wrap(
            spacing: 12,
            runSpacing: 12,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: <Widget>[
              WabButton(
                key: const ValueKey<String>('write'),
                kind: screenplay == null ? WabMaterialKind.seal : WabMaterialKind.zhuwen,
                onPressed: canWrite ? onWrite : null,
                child: Text(screenplay == null ? 'Write screenplay' : 'Write a new draft'),
              ),
              Text('${model.kind.label} · ${documentCostLabel(model, screenplayCostMicros)}'),
              if (screenplay != null && !documents.screenplayLocked)
                WabButton(
                  key: const ValueKey<String>('lock'),
                  kind: WabMaterialKind.seal,
                  onPressed: _idle ? () => onLock(screenplay.revision) : null,
                  child: Text('Lock revision ${screenplay.revision}'),
                ),
            ],
          ),
        ],
      ),
    );
  }

  Widget _scene(BuildContext context, KinetoScreenplay screenplay, int index) {
    final scene = screenplay.scenes[index];
    return WabPanel(
      title: '${index + 1}. ${scene.heading}',
      trailing: Text(scene.setting.label),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          Text(scene.dramaticGoal, style: const TextStyle(fontStyle: FontStyle.italic)),
          const SizedBox(height: 16),
          for (final block in scene.blocks) ...<Widget>[
            _block(context, screenplay, block),
            const SizedBox(height: 12),
          ],
          Wrap(
            spacing: 12,
            children: <Widget>[
              WabButton(
                key: ValueKey<String>('edit-${scene.id}'),
                kind: WabMaterialKind.zhuwen,
                onPressed: _idle ? () => onEdit(scene.id) : null,
                child: const Text('Edit'),
              ),
              WabButton(
                key: ValueKey<String>('rewrite-${scene.id}'),
                kind: WabMaterialKind.zhuwen,
                onPressed: _idle && model.ready ? () => _rewriteDialog(context, scene) : null,
                child: const Text('Rewrite…'),
              ),
            ],
          ),
        ],
      ),
    );
  }

  Widget _block(BuildContext context, KinetoScreenplay screenplay, KinetoScreenplayBlock block) =>
      switch (block.kind) {
        KinetoBlockKind.action => Text(block.text),
        KinetoBlockKind.transition => Align(
            alignment: Alignment.centerRight,
            child: Text(block.text.toUpperCase(), style: const TextStyle(fontStyle: FontStyle.italic)),
          ),
        KinetoBlockKind.dialogue => Padding(
            padding: const EdgeInsets.symmetric(horizontal: 96),
            child: Column(
              children: <Widget>[
                Text(
                  screenplay.nameOf(block.character ?? '').toUpperCase(),
                  style: const TextStyle(fontWeight: FontWeight.bold),
                ),
                if (block.parenthetical != null)
                  Text('(${block.parenthetical})', style: const TextStyle(fontStyle: FontStyle.italic)),
                Text(block.text, textAlign: TextAlign.center),
              ],
            ),
          ),
      };

  Widget _history() => WabPanel(
        title: 'Revisions',
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: <Widget>[
            for (final revision in documents.revisions.reversed)
              Padding(
                padding: const EdgeInsets.only(bottom: 6),
                child: Text(describeRevision(revision)),
              ),
          ],
        ),
      );

  Future<void> _importDialog(BuildContext context) async {
    final text = await showDialog<String>(
      context: context,
      builder: (context) => const TextPromptDialog(
        title: 'Import story text',
        width: 640,
        fieldKey: 'import-text',
        minLines: 10,
        maxLines: 20,
        hint: 'Paste the story to adapt',
        confirmKey: 'import-confirm',
        confirmLabel: 'Import as a new source revision',
      ),
    );
    if (text != null && text.trim().isNotEmpty) onImport(text);
  }

  Future<void> _rewriteDialog(BuildContext context, KinetoScene scene) async {
    final direction = await showDialog<String>(
      context: context,
      builder: (context) => TextPromptDialog(
        title: 'Rewrite “${scene.heading}”',
        width: 560,
        fieldKey: 'rewrite-direction',
        minLines: 2,
        maxLines: 5,
        hint: 'What should change? Leave empty for a fresh take.',
        confirmKey: 'rewrite-confirm',
        confirmLabel: 'Rewrite · ${documentCostLabel(model, null)}',
      ),
    );
    if (direction == null) return;
    final trimmed = direction.trim();
    onRewrite(scene.id, trimmed.isEmpty ? null : trimmed);
  }
}

/// A dialog asking for one piece of text. It owns its controller: the field
/// is still built while the dialog animates out, so the controller may only
/// be disposed with the dialog itself.
class TextPromptDialog extends StatefulWidget {
  const TextPromptDialog({
    super.key,
    required this.title,
    required this.width,
    required this.fieldKey,
    required this.minLines,
    required this.maxLines,
    required this.hint,
    required this.confirmKey,
    required this.confirmLabel,
  });

  final String title;
  final double width;
  final String fieldKey;
  final int minLines;
  final int maxLines;
  final String hint;
  final String confirmKey;
  final String confirmLabel;

  @override
  State<TextPromptDialog> createState() => _TextPromptDialogState();
}

class _TextPromptDialogState extends State<TextPromptDialog> {
  final TextEditingController _controller = TextEditingController();

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
        title: Text(widget.title),
        content: SizedBox(
          width: widget.width,
          child: TextField(
            key: ValueKey<String>(widget.fieldKey),
            controller: _controller,
            minLines: widget.minLines,
            maxLines: widget.maxLines,
            decoration: wabInputDecoration(hintText: widget.hint),
          ),
        ),
        actions: <Widget>[
          TextButton(onPressed: () => Navigator.pop(context), child: const Text('Cancel')),
          TextButton(
            key: ValueKey<String>(widget.confirmKey),
            onPressed: () => Navigator.pop(context, _controller.text),
            child: Text(widget.confirmLabel),
          ),
        ],
      );
}

/// One line of the revision history.
String describeRevision(KinetoRevision revision) {
  final origin = switch (revision.origin) {
    KinetoRevisionOrigin.edited => 'edited from revision ${revision.basedOn}',
    KinetoRevisionOrigin.generated when revision.basedOn != null =>
      'scene rewritten from revision ${revision.basedOn} by ${revision.provider}',
    KinetoRevisionOrigin.generated => 'written by ${revision.provider}'
        '${revision.model.isEmpty || revision.model == 'agent-default' ? '' : ' (${revision.model})'}',
  };
  final status = switch (revision.status) {
    KinetoRevisionStatus.locked => ' · locked',
    KinetoRevisionStatus.candidate => ' · draft',
    KinetoRevisionStatus.superseded => '',
  };
  return 'Revision ${revision.number} · $origin$status';
}

/// Edits one scene: its heading, time, dramatic goal and blocks.
class SceneEditor extends StatefulWidget {
  const SceneEditor({
    required this.number,
    required this.scene,
    required this.cast,
    required this.onCancel,
    required this.onSave,
    this.working = false,
    super.key,
  });

  final int number;
  final KinetoScene scene;
  final List<KinetoCastMember> cast;
  final bool working;
  final VoidCallback onCancel;
  final ValueChanged<SceneDraft> onSave;

  @override
  State<SceneEditor> createState() => _SceneEditorState();
}

final class _BlockDraft {
  _BlockDraft(KinetoScreenplayBlock block)
      : kind = block.kind,
        character = block.character,
        parenthetical = TextEditingController(text: block.parenthetical ?? ''),
        text = TextEditingController(text: block.text);

  KinetoBlockKind kind;
  String? character;
  final TextEditingController parenthetical;
  final TextEditingController text;

  void dispose() {
    parenthetical.dispose();
    text.dispose();
  }
}

class _SceneEditorState extends State<SceneEditor> {
  late final TextEditingController _heading = TextEditingController(text: widget.scene.heading);
  late final TextEditingController _time = TextEditingController(text: widget.scene.time);
  late final TextEditingController _goal = TextEditingController(text: widget.scene.dramaticGoal);
  late final List<_BlockDraft> _blocks = widget.scene.blocks.map(_BlockDraft.new).toList();
  String? _problem;

  @override
  void dispose() {
    _heading.dispose();
    _time.dispose();
    _goal.dispose();
    for (final block in _blocks) {
      block.dispose();
    }
    super.dispose();
  }

  void _save() {
    final blocks = <KinetoScreenplayBlock>[];
    for (final block in _blocks) {
      final text = block.text.text.trim();
      // An emptied block is a deleted one.
      if (text.isEmpty) continue;
      final dialogue = block.kind == KinetoBlockKind.dialogue;
      if (dialogue && block.character == null) {
        setState(() => _problem = 'Choose who speaks every line of dialogue.');
        return;
      }
      final parenthetical = block.parenthetical.text.trim();
      blocks.add(
        KinetoScreenplayBlock(
          kind: block.kind,
          character: dialogue ? block.character : null,
          parenthetical: dialogue && parenthetical.isNotEmpty ? parenthetical : null,
          text: text,
        ),
      );
    }
    if (blocks.isEmpty || _heading.text.trim().isEmpty || _time.text.trim().isEmpty || _goal.text.trim().isEmpty) {
      setState(() => _problem = 'A scene needs a heading, a time, a dramatic goal and at least one block.');
      return;
    }
    setState(() => _problem = null);
    widget.onSave(
      SceneDraft(
        sceneId: widget.scene.id,
        heading: _heading.text.trim(),
        time: _time.text.trim(),
        dramaticGoal: _goal.text.trim(),
        blocks: blocks,
      ),
    );
  }

  Widget _field(String key, String label, TextEditingController controller, {int lines = 1}) => Padding(
        padding: const EdgeInsets.only(bottom: 12),
        child: TextField(
          key: ValueKey<String>(key),
          controller: controller,
          minLines: lines,
          maxLines: lines == 1 ? 1 : lines + 4,
          style: TextStyle(color: WabTheme.textColor),
          decoration: wabInputDecoration(hintText: label),
        ),
      );

  @override
  Widget build(BuildContext context) {
    final id = widget.scene.id;
    return WabPanel(
      title: '${widget.number}. Editing ${widget.scene.heading}',
      trailing: const WabStatusBadge('Editing', kind: WabBadgeKind.warning),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          _field('scene-$id-heading', 'Scene heading', _heading),
          _field('scene-$id-time', 'Time', _time),
          _field('scene-$id-goal', 'Dramatic goal', _goal, lines: 2),
          for (var index = 0; index < _blocks.length; index++) _blockEditor(index),
          Wrap(
            spacing: 12,
            runSpacing: 12,
            children: <Widget>[
              for (final kind in KinetoBlockKind.values)
                WabButton(
                  kind: WabMaterialKind.paper,
                  onPressed: () => setState(
                    () => _blocks.add(_BlockDraft(KinetoScreenplayBlock(kind: kind, text: ''))),
                  ),
                  child: Text('Add ${kind.label.toLowerCase()}'),
                ),
            ],
          ),
          if (_problem != null) ...<Widget>[const SizedBox(height: 12), Text(_problem!)],
          const SizedBox(height: 16),
          Wrap(
            spacing: 12,
            children: <Widget>[
              WabButton(
                key: ValueKey<String>('save-$id'),
                kind: WabMaterialKind.seal,
                onPressed: widget.working ? null : _save,
                child: const Text('Save as a new revision'),
              ),
              WabButton(
                kind: WabMaterialKind.zhuwen,
                onPressed: widget.working ? null : widget.onCancel,
                child: const Text('Cancel'),
              ),
            ],
          ),
        ],
      ),
    );
  }

  Widget _blockEditor(int index) {
    final block = _blocks[index];
    final id = widget.scene.id;
    return Padding(
      padding: const EdgeInsets.only(bottom: 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          Wrap(
            spacing: 12,
            runSpacing: 8,
            children: <Widget>[
              SizedBox(
                width: 200,
                child: WabDropdown<KinetoBlockKind>(
                  key: ValueKey<String>('block-$id-$index-kind-${block.kind.name}'),
                  value: block.kind,
                  items: <KinetoBlockKind, String>{
                    for (final kind in KinetoBlockKind.values) kind: kind.label,
                  },
                  onChanged: (kind) => setState(() => block.kind = kind ?? block.kind),
                ),
              ),
              if (block.kind == KinetoBlockKind.dialogue) ...<Widget>[
                SizedBox(
                  width: 240,
                  child: WabDropdown<String>(
                    key: ValueKey<String>('block-$id-$index-character'),
                    value: block.character,
                    hintText: 'Who speaks',
                    items: <String, String>{
                      for (final member in widget.cast) member.id: member.name,
                    },
                    onChanged: (character) => setState(() => block.character = character),
                  ),
                ),
                SizedBox(
                  width: 240,
                  child: TextField(
                    key: ValueKey<String>('block-$id-$index-parenthetical'),
                    controller: block.parenthetical,
                    style: TextStyle(color: WabTheme.textColor),
                    decoration: wabInputDecoration(hintText: 'Direction for the line'),
                  ),
                ),
              ],
            ],
          ),
          const SizedBox(height: 8),
          TextField(
            key: ValueKey<String>('block-$id-$index-text'),
            controller: block.text,
            minLines: 1,
            maxLines: 8,
            style: TextStyle(color: WabTheme.textColor),
            decoration: wabInputDecoration(hintText: '${block.kind.label} (empty removes it)'),
          ),
        ],
      ),
    );
  }
}
