import 'package:flutter/material.dart';
import 'package:wabisabi/wabisabi.dart';

import 'kineto_controls.dart';
import 'kineto_work.dart';
import 'native/kineto_engine.dart';
import 'screenplay_workspace.dart';

/// The recipe's steps, each with where it stands.
class RecipeStrip extends StatelessWidget {
  const RecipeStrip({required this.steps, this.problem, super.key});

  final List<KinetoWorkflowStep> steps;
  final String? problem;

  @override
  Widget build(BuildContext context) {
    if (problem != null) {
      return WabPanel(title: 'Workflow', child: Text(problem!));
    }
    return Wrap(
      spacing: 8,
      runSpacing: 8,
      children: <Widget>[
        for (final step in steps)
          Tooltip(
            message: <String>[
              step.description,
              if (step.waitingOn.isNotEmpty) 'Waiting on ${step.waitingOn.join(', ')}',
              if (step.stale.isNotEmpty) 'Changed since: ${step.stale.join(', ')}',
              if (step.skippedWhen.isNotEmpty)
                'Skipped: ${step.skippedWhen.map((condition) => condition.replaceAll('_', ' ')).join(', ')}',
            ].join('\n'),
            child: WabStatusBadge(
              '${step.id.replaceAll('_', ' ')} · ${step.state.label}${step.stale.isEmpty ? '' : ' · stale'}',
              key: ValueKey<String>('step-${step.id}'),
              kind: step.stale.isNotEmpty
                  ? WabBadgeKind.warning
                  : switch (step.state) {
                      KinetoStepState.done => WabBadgeKind.done,
                      KinetoStepState.awaitingApproval => WabBadgeKind.progress,
                      KinetoStepState.ready => WabBadgeKind.primary,
                      KinetoStepState.waiting || KinetoStepState.skipped => WabBadgeKind.neutral,
                    },
            ),
          ),
      ],
    );
  }
}

class ShotPlanWorkspaceScreen extends StatefulWidget {
  const ShotPlanWorkspaceScreen({
    required this.work,
    required this.project,
    this.navigation,
    this.isDarkMode = true,
    this.onThemeToggle,
    super.key,
  });

  final KinetoProjectSession project;
  final Widget? navigation;
  final bool isDarkMode;
  final VoidCallback? onThemeToggle;

  /// The project's requests, which outlive this page.
  final KinetoWork work;

  @override
  State<ShotPlanWorkspaceScreen> createState() => _ShotPlanWorkspaceScreenState();
}

class _ShotPlanWorkspaceScreenState extends State<ShotPlanWorkspaceScreen> {
  late final KinetoLanguageModel _model;
  late KinetoPlanning _planning;
  late bool _ready;
  int? _cost;
  static const String _page = 'shotPlan';
  int _finished = 0;

  // The project's requests outlive this page: what is under way, and how
  // the last one asked for here ended, come from them.
  // Requests asked for here change what this page shows, one after another
  // in the engine; requests asked for elsewhere do not hold it up.
  bool get _working => widget.work.busyOn(_page);
  String get _workingLabel => widget.work.latestOn(_page)?.label ?? 'Working';
  String? get _error => widget.work.problemOn(_page);
  VoidCallback? get _resubmit => widget.work.retryOn(_page);
  String? _editingScene;

  @override
  void initState() {
    super.initState();
    _finished = widget.work.finished;
    widget.work.addListener(_workChanged);
    _model = widget.project.languageModel;
    _read();
  }

  void _read() {
    _planning = widget.project.planning;
    final production = widget.project.production;
    _ready = widget.project.documents.screenplayLocked && production.bibleIsLocked;
    _cost = widget.project.documentCostMicros(KinetoDocumentKind.shotPlan);
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

  Future<void> _run(
    Future<void> Function() action,
    String label, {
    VoidCallback? onDone,
    Future<void> Function()? resubmit,
  }) async {
    final done = await widget.work.run(_page, label, action, retry: resubmit);
    if (done && mounted) onDone?.call();
  }

  @override
  Widget build(BuildContext context) {
    final project = widget.project;
    final asking = 'with ${_model.kind.label}';
    return ShotPlanWorkspaceView(
      planning: _planning,
      model: _model,
      ready: _ready,
      costMicros: _cost,
      working: _working,
      workingLabel: _workingLabel,
      error: _error,
      onResubmit: _resubmit,
      editingScene: _editingScene,
      navigation: widget.navigation,
      isDarkMode: widget.isDarkMode,
      onThemeToggle: widget.onThemeToggle,
      onPlan: () => _run(
        project.planShots,
        'Planning the shots $asking',
        resubmit: () => project.planShots(resubmit: true),
      ),
      onReplan: (scene, direction) => _run(
        () => project.replanScene(scene, direction: direction),
        'Planning the scene again $asking',
        resubmit: () => project.replanScene(scene, direction: direction, resubmit: true),
      ),
      onEdit: (scene) => setState(() {
        _editingScene = scene;
        widget.work.dismiss();
      }),
      onSave: (scene) => _run(
        () => project.saveScenePlan(_planning.revision!, scene),
        'Saving',
        onDone: () => _editingScene = null,
      ),
      onLock: (revision) => _run(() => project.lockShotPlan(revision), 'Locking'),
    );
  }
}

class ShotPlanWorkspaceView extends StatelessWidget {
  const ShotPlanWorkspaceView({
    required this.planning,
    required this.model,
    required this.onPlan,
    required this.onReplan,
    required this.onEdit,
    required this.onSave,
    required this.onLock,
    this.ready = false,
    this.costMicros,
    this.working = false,
    this.workingLabel = 'Working',
    this.error,
    this.onResubmit,
    this.editingScene,
    this.navigation,
    this.isDarkMode = true,
    this.onThemeToggle,
    super.key,
  });

  final KinetoPlanning planning;
  final KinetoLanguageModel model;

  /// The screenplay and the bible are locked, so shots can be planned.
  final bool ready;
  final int? costMicros;
  final bool working;
  final String workingLabel;
  final String? error;
  final VoidCallback? onResubmit;

  /// The scene whose shots are being edited, if any.
  final String? editingScene;
  final Widget? navigation;
  final bool isDarkMode;
  final VoidCallback? onThemeToggle;

  final VoidCallback onPlan;
  final void Function(String scene, String? direction) onReplan;
  final ValueChanged<String?> onEdit;
  final ValueChanged<KinetoScenePlan> onSave;
  final ValueChanged<int> onLock;

  bool get _idle => !working && editingScene == null;

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
                  RecipeStrip(steps: planning.steps, problem: planning.workflowProblem),
                  const SizedBox(height: 20),
                  if (error != null) ...<Widget>[_errorPanel(), const SizedBox(height: 20)],
                  _planPanel(context),
                  for (final scene in planning.scenes) ...<Widget>[
                    const SizedBox(height: 20),
                    scene.sceneId == editingScene
                        ? ScenePlanEditor(
                            key: ValueKey<String>('plan-editor-${planning.revision}-${scene.sceneId}'),
                            scene: scene,
                            earlierShots: <String>[
                              for (final earlier in planning.scenes.takeWhile((other) => other != scene))
                                for (final shot in earlier.shots) shot.id,
                            ],
                            otherShots: <String>{
                              for (final other in planning.scenes)
                                if (other != scene)
                                  for (final shot in other.shots) shot.id,
                            },
                            working: working,
                            onCancel: () => onEdit(null),
                            onSave: onSave,
                          )
                        : _scene(context, scene),
                  ],
                ],
              ),
            ),
          ),
        ),
      );

  Widget _errorPanel() => WabPanel(
        title: 'That did not work',
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: <Widget>[
            Text(error!),
            if (onResubmit != null) ...<Widget>[
              const SizedBox(height: 12),
              const Text(
                'The model may have answered the last request, and charged for it, without the '
                'answer reaching Kineto. Check your account before asking again.',
              ),
              const SizedBox(height: 12),
              KinetoButton(
                key: const ValueKey<String>('resubmit-plan'),
                onPressed: working ? null : onResubmit,
                child: const Text('Ask again · may pay twice'),
              ),
            ],
          ],
        ),
      );

  Widget _planPanel(BuildContext context) {
    final problem = languageModelProblem(model);
    final (status, kind) = planning.revision == null
        ? ('None yet', WabBadgeKind.neutral)
        : planning.planLocked
            ? (planning.stale.isEmpty ? 'Locked' : 'Locked · changed since', WabBadgeKind.done)
            : planning.stale.isNotEmpty
                ? ('Changed since', WabBadgeKind.warning)
                : ('Draft', WabBadgeKind.progress);
    return WabPanel(
      title: planning.revision == null ? 'Shot plan' : 'Shot plan · revision ${planning.revision}',
      trailing: WabStatusBadge(status, kind: kind),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          if (planning.revision == null)
            Text(
              ready
                  ? 'Not planned yet. The plan breaks every scene into shots: what each is for, how '
                      'it is framed and moves, who is in it and what must carry over.'
                  : 'Lock the screenplay and the bible first; shots are planned from both.',
            ),
          if (planning.stale.isNotEmpty) ...<Widget>[
            Text('May no longer match: ${planning.stale.map((cause) => cause.description).join('; ')}.'),
            const SizedBox(height: 8),
          ],
          if (planning.revision != null && !planning.planLocked)
            const Text('Locking the plan puts its shots in play in the shots workspace.'),
          const SizedBox(height: 12),
          if (problem != null) ...<Widget>[Text(problem), const SizedBox(height: 12)],
          Wrap(
            spacing: 12,
            runSpacing: 12,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: <Widget>[
              KinetoButton(
                key: const ValueKey<String>('plan-shots'),
                primary: true,
                onPressed: _idle && ready && model.ready ? onPlan : null,
                child: Text(planning.revision == null ? 'Plan the shots' : 'Plan them again'),
              ),
              if (planning.revision != null && !planning.planLocked)
                KinetoButton(
                  key: const ValueKey<String>('lock-plan'),
                  primary: true,
                  onPressed: _idle ? () => onLock(planning.revision!) : null,
                  child: Text('Lock revision ${planning.revision}'),
                ),
              Text('${model.kind.label} · ${documentCostLabel(model, costMicros)}'),
            ],
          ),
          if (planning.revisions.isNotEmpty) ...<Widget>[
            const SizedBox(height: 12),
            for (final revision in planning.revisions.reversed) Text(describeRevision(revision)),
          ],
        ],
      ),
    );
  }

  Widget _scene(BuildContext context, KinetoScenePlan scene) => WabPanel(
        title: scene.sceneId.replaceAll('_', ' '),
        trailing: Text('${scene.shots.length} shots'),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: <Widget>[
            for (final shot in scene.shots) _shot(context, shot),
            Wrap(
              spacing: 12,
              runSpacing: 12,
              children: <Widget>[
                KinetoButton(
                  key: ValueKey<String>('edit-scene-${scene.sceneId}'),
                  onPressed: _idle ? () => onEdit(scene.sceneId) : null,
                  child: const Text('Edit shots'),
                ),
                KinetoButton(
                  key: ValueKey<String>('replan-${scene.sceneId}'),
                  onPressed: _idle && model.ready ? () => _replanDialog(context, scene.sceneId) : null,
                  child: const Text('Plan this scene again'),
                ),
              ],
            ),
          ],
        ),
      );

  Widget _shot(BuildContext context, KinetoPlannedShot shot) => Padding(
        padding: const EdgeInsets.only(bottom: 16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: <Widget>[
            Text(
              '${shot.id} · ${shot.framing.label} · ${shot.lens} · ${shot.cameraMotion.label} · '
              '${shot.durationTarget.toStringAsFixed(1)} s',
              style: Theme.of(context).textTheme.titleSmall,
            ),
            Text(shot.purpose),
            Text(shot.action),
            if (shot.dialogue.isNotEmpty)
              Text(
                '“${shot.dialogue}”'
                '${shot.dialogueMillis == 0 ? '' : ' · ${_seconds(shot.dialogueMillis)} s to say'}',
              ),
            if (shot.tooShort)
              WabStatusBadge(
                'Too short for its dialogue: ${_seconds(shot.dialogueMillis)} s to say in '
                '${shot.durationTarget.toStringAsFixed(1)} s',
                key: ValueKey<String>('too-short-${shot.id}'),
                kind: WabBadgeKind.warning,
              ),
            Text('Mood: ${shot.emotion} · ${shot.direction.label}'),
            if (shot.characters.isNotEmpty) Text('With ${shot.characters.join(', ')}'),
            if (shot.continuityFrom != null) Text('Continues from ${shot.continuityFrom}'),
            for (final note in shot.continuity) Text('Keep: $note'),
          ],
        ),
      );

  Future<void> _replanDialog(BuildContext context, String sceneId) async {
    final direction = await showDialog<String>(
      context: context,
      builder: (context) => TextPromptDialog(
        title: 'Plan ${sceneId.replaceAll('_', ' ')} again',
        width: 560,
        fieldKey: 'replan-direction',
        minLines: 2,
        maxLines: 5,
        hint: 'What should change? Leave empty for a fresh plan of the scene.',
        confirmKey: 'replan-confirm',
        confirmLabel: 'Plan again · ${documentCostLabel(model, costMicros)}',
      ),
    );
    if (direction == null) return;
    final trimmed = direction.trim();
    onReplan(sceneId, trimmed.isEmpty ? null : trimmed);
  }
}

/// Edits one scene's shots: which shots there are and in what order, what
/// each is for, how it is framed and moves, what happens and is said, the
/// mood, how long it runs and which earlier shot it continues.
class ScenePlanEditor extends StatefulWidget {
  const ScenePlanEditor({
    required this.scene,
    required this.onCancel,
    required this.onSave,
    this.earlierShots = const <String>[],
    this.otherShots = const <String>{},
    this.working = false,
    super.key,
  });

  final KinetoScenePlan scene;

  /// The shots of the scenes before this one, in plan order: a shot may
  /// continue from any of them.
  final List<String> earlierShots;

  /// Every shot ID of the other scenes, which a new shot must not take.
  final Set<String> otherShots;
  final bool working;
  final VoidCallback onCancel;
  final ValueChanged<KinetoScenePlan> onSave;

  @override
  State<ScenePlanEditor> createState() => _ScenePlanEditorState();
}

final class _ShotDraft {
  _ShotDraft(this.shot)
      : framing = shot.framing,
        cameraMotion = shot.cameraMotion,
        continuityFrom = shot.continuityFrom,
        purpose = TextEditingController(text: shot.purpose),
        action = TextEditingController(text: shot.action),
        dialogue = TextEditingController(text: shot.dialogue),
        emotion = TextEditingController(text: shot.emotion),
        lens = TextEditingController(text: shot.lens),
        duration = TextEditingController(text: shot.durationTarget.toString());

  final KinetoPlannedShot shot;
  KinetoFraming framing;
  KinetoCameraMotion cameraMotion;
  String? continuityFrom;
  final TextEditingController purpose;
  final TextEditingController action;
  final TextEditingController dialogue;
  final TextEditingController emotion;
  final TextEditingController lens;
  final TextEditingController duration;

  void dispose() {
    for (final controller in <TextEditingController>[purpose, action, dialogue, emotion, lens, duration]) {
      controller.dispose();
    }
  }
}

class _ScenePlanEditorState extends State<ScenePlanEditor> {
  late final List<_ShotDraft> _shots = widget.scene.shots.map(_ShotDraft.new).toList();
  String? _problem;

  @override
  void dispose() {
    for (final shot in _shots) {
      shot.dispose();
    }
    super.dispose();
  }

  void _save() {
    final shots = <KinetoPlannedShot>[];
    for (final draft in _shots) {
      final duration = double.tryParse(draft.duration.text.trim());
      final purpose = draft.purpose.text.trim();
      final action = draft.action.text.trim();
      final emotion = draft.emotion.text.trim();
      final lens = draft.lens.text.trim();
      if (duration == null || purpose.isEmpty || action.isEmpty || emotion.isEmpty || lens.isEmpty) {
        setState(() => _problem = 'Every shot needs a purpose, an action, a mood, a lens and a duration in seconds.');
        return;
      }
      shots.add(
        KinetoPlannedShot(
          id: draft.shot.id,
          purpose: purpose,
          direction: draft.shot.direction,
          framing: draft.framing,
          lens: lens,
          cameraMotion: draft.cameraMotion,
          action: action,
          dialogue: draft.dialogue.text.trim(),
          emotion: emotion,
          durationTarget: duration,
          characters: draft.shot.characters,
          continuity: draft.shot.continuity,
          continuityFrom: draft.continuityFrom,
          added: draft.shot.added,
        ),
      );
    }
    setState(() => _problem = null);
    widget.onSave(KinetoScenePlan(sceneId: widget.scene.sceneId, shots: shots));
  }

  /// The shots the shot at [index] may continue from: every shot before it.
  List<String> _before(int index) => <String>[
        ...widget.earlierShots,
        for (final draft in _shots.take(index)) draft.shot.id,
      ];

  /// After shots move or go, a shot continues only from one still before it.
  void _restructure(void Function() change) => setState(() {
        change();
        for (var index = 0; index < _shots.length; index++) {
          final from = _shots[index].continuityFrom;
          if (from != null && !_before(index).contains(from)) _shots[index].continuityFrom = null;
        }
      });

  void _move(int index, int offset) => _restructure(() {
        final draft = _shots.removeAt(index);
        _shots.insert(index + offset, draft);
      });

  void _remove(int index) => _restructure(() => _shots.removeAt(index).dispose());

  /// A new shot after the one at [index], staged like it and still to be
  /// written: what it is for, what happens and its mood.
  void _addAfter(int index) => _restructure(() {
        final like = _shots[index].shot;
        _shots.insert(
          index + 1,
          _ShotDraft(
            KinetoPlannedShot(
              id: _newShotId(),
              purpose: '',
              direction: like.direction,
              framing: like.framing,
              lens: like.lens,
              cameraMotion: like.cameraMotion,
              action: '',
              emotion: '',
              durationTarget: like.durationTarget,
              characters: like.characters,
              added: true,
            ),
          ),
        );
      });

  /// The scene's ID and the first number free of every shot the scene had
  /// when editing began, so a shot removed in this edit keeps its number.
  /// Never `shot_NNN`, which names the bootstrap scene's shots. The shot is
  /// sent as added: if its number was a shot's before, even one removed in
  /// an earlier revision, the engine gives it one no shot has had, so it
  /// never inherits another shot's takes or video.
  String _newShotId() {
    final taken = <String>{
      ...widget.otherShots,
      for (final shot in widget.scene.shots) shot.id,
      for (final draft in _shots) draft.shot.id,
    };
    final scene = widget.scene.sceneId;
    final prefix = scene.length > 60 ? scene.substring(0, 60) : scene;
    for (var number = _shots.length + 1;; number++) {
      final id = '${prefix}_${number.toString().padLeft(2, '0')}';
      if (!taken.contains(id)) return id;
    }
  }

  Widget _field(String key, String label, TextEditingController controller, {int lines = 1}) => Padding(
        padding: const EdgeInsets.only(bottom: 8),
        child: TextField(
          key: ValueKey<String>(key),
          controller: controller,
          minLines: lines,
          maxLines: lines + 3,
          style: TextStyle(color: WabTheme.textColor),
          decoration: wabInputDecoration(hintText: label),
        ),
      );

  @override
  Widget build(BuildContext context) => WabPanel(
        title: 'Editing ${widget.scene.sceneId.replaceAll('_', ' ')}',
        trailing: const WabStatusBadge('Editing', kind: WabBadgeKind.warning),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: <Widget>[
            for (final (index, draft) in _shots.indexed)
              KeyedSubtree(
                key: ObjectKey(draft),
                child: _shot(context, index, draft),
              ),
            if (_problem != null) ...<Widget>[Text(_problem!), const SizedBox(height: 12)],
            Wrap(
              spacing: 12,
              children: <Widget>[
                KinetoButton(
                  key: ValueKey<String>('save-scene-plan-${widget.scene.sceneId}'),
                  primary: true,
                  onPressed: widget.working ? null : _save,
                  child: const Text('Save as a new revision'),
                ),
                KinetoButton(
                  onPressed: widget.working ? null : widget.onCancel,
                  child: const Text('Cancel'),
                ),
              ],
            ),
          ],
        ),
      );

  Widget _shot(BuildContext context, int index, _ShotDraft draft) {
    final id = draft.shot.id;
    final before = _before(index);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: <Widget>[
        Wrap(
          spacing: 8,
          runSpacing: 8,
          crossAxisAlignment: WrapCrossAlignment.center,
          children: <Widget>[
            Text('${index + 1}. $id', style: Theme.of(context).textTheme.titleSmall),
            KinetoButton(
              key: ValueKey<String>('shot-$id-up'),
              onPressed: index > 0 ? () => _move(index, -1) : null,
              child: const Text('Earlier'),
            ),
            KinetoButton(
              key: ValueKey<String>('shot-$id-down'),
              onPressed: index < _shots.length - 1 ? () => _move(index, 1) : null,
              child: const Text('Later'),
            ),
            KinetoButton(
              key: ValueKey<String>('shot-$id-add-after'),
              onPressed: () => _addAfter(index),
              child: const Text('Add a shot after'),
            ),
            KinetoButton(
              key: ValueKey<String>('shot-$id-remove'),
              onPressed: _shots.length > 1 ? () => _remove(index) : null,
              child: const Text('Remove'),
            ),
          ],
        ),
        const SizedBox(height: 8),
        _field('shot-$id-purpose', 'What the shot is for', draft.purpose),
        _field('shot-$id-action', 'What happens', draft.action, lines: 2),
        _field('shot-$id-dialogue', 'What is said (may be empty)', draft.dialogue),
        if (draft.shot.dialogueMillis > 0)
          Padding(
            padding: const EdgeInsets.only(bottom: 8),
            child: Text(
              'As planned, its dialogue takes ${_seconds(draft.shot.dialogueMillis)} s to say; '
              'keep the shot at least that long.',
              key: ValueKey<String>('shot-$id-dialogue-length'),
            ),
          ),
        _field('shot-$id-emotion', 'The mood', draft.emotion),
        Wrap(
          spacing: 12,
          runSpacing: 8,
          children: <Widget>[
            SizedBox(
              width: 300,
              child: WabDropdown<KinetoFraming>(
                key: ValueKey<String>('shot-$id-framing'),
                value: draft.framing,
                items: <KinetoFraming, String>{
                  for (final framing in KinetoFraming.values) framing: framing.label,
                },
                onChanged: (framing) => setState(() => draft.framing = framing ?? draft.framing),
              ),
            ),
            SizedBox(
              width: 300,
              child: WabDropdown<KinetoCameraMotion>(
                key: ValueKey<String>('shot-$id-motion'),
                value: draft.cameraMotion,
                items: <KinetoCameraMotion, String>{
                  for (final motion in KinetoCameraMotion.values) motion: motion.label,
                },
                onChanged: (motion) => setState(() => draft.cameraMotion = motion ?? draft.cameraMotion),
              ),
            ),
            SizedBox(width: 120, child: _field('shot-$id-lens', 'Lens', draft.lens)),
            SizedBox(width: 120, child: _field('shot-$id-duration', 'Seconds', draft.duration)),
          ],
        ),
        const SizedBox(height: 8),
        Row(
          children: <Widget>[
            const Text('Continues from'),
            const SizedBox(width: 12),
            Expanded(
              key: ValueKey<String>('shot-$id-from'),
              // Rebuilt whenever what it offers changes: a form field keeps
              // the value it started with.
              child: WabDropdown<String>(
                key: ValueKey<String>('${before.join(',')}|${draft.continuityFrom}'),
                value: draft.continuityFrom ?? '',
                items: <String, String>{
                  '': 'no earlier shot',
                  for (final earlier in before) earlier: earlier,
                },
                onChanged: (from) =>
                    setState(() => draft.continuityFrom = from == null || from.isEmpty ? null : from),
              ),
            ),
          ],
        ),
        const SizedBox(height: 16),
      ],
    );
  }
}

/// [millis] as seconds with one decimal, for example `1.4`.
String _seconds(int millis) => (millis / 1000).toStringAsFixed(1);
