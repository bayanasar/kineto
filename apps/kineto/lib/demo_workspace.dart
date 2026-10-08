import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:wabisabi/wabisabi.dart';

import 'native/kineto_engine.dart';
import 'screenplay_workspace.dart' show TextPromptDialog;

final class DemoCandidateContent {
  const DemoCandidateContent({
    required this.title,
    required this.description,
    required this.icon,
  });

  final String title;
  final String description;
  final IconData icon;
}

final class DemoShotContent {
  const DemoShotContent({
    required this.label,
    required this.setting,
    required this.description,
    required this.candidates,
  });

  final String label;
  final String setting;
  final String description;
  final List<DemoCandidateContent> candidates;
}

const List<DemoShotContent> demoShots = <DemoShotContent>[
  DemoShotContent(
    label: 'Shot 001',
    setting: 'Interior · Night',
    description:
        'A filmmaker waits across the table. The final line lands, and the reaction matters more than the dialogue.',
    candidates: <DemoCandidateContent>[
      DemoCandidateContent(
        title: 'Wide master',
        description: '35 mm · static frame · establishes the room and both performers.',
        icon: Icons.panorama_wide_angle,
      ),
      DemoCandidateContent(
        title: 'Profile medium',
        description: '50 mm · slow dolly in · keeps the exchange intimate but spatially clear.',
        icon: Icons.person_outline,
      ),
      DemoCandidateContent(
        title: 'Close-up',
        description: '85 mm · locked frame · prioritizes the final reaction and eye line.',
        icon: Icons.face_outlined,
      ),
    ],
  ),
  DemoShotContent(
    label: 'Shot 002',
    setting: 'Interior · Night · Doorway',
    description:
        'The filmmaker stands to leave. The second beat needs to preserve geography while carrying the tension into the doorway.',
    candidates: <DemoCandidateContent>[
      DemoCandidateContent(
        title: 'Doorway two-shot',
        description: '40 mm · restrained pan · keeps both performers and the exit in one readable frame.',
        icon: Icons.sensor_door_outlined,
      ),
      DemoCandidateContent(
        title: 'Over shoulder',
        description: '65 mm · shoulder foreground · compresses distance as the conversation breaks.',
        icon: Icons.switch_account_outlined,
      ),
      DemoCandidateContent(
        title: 'Exit detail',
        description: '90 mm · static insert · isolates the hand on the door before the cut.',
        icon: Icons.pan_tool_alt_outlined,
      ),
    ],
  ),
];

/// Plain-language notes for the work an open recovered, one per recovery.
List<String> describeRecoveries(List<KinetoRecovery> recoveries) => <String>[
      for (final recovery in recoveries)
        if (recovery.needsAttention)
          '${recovery.revisionOrCount} job record(s) could not be read and were left untouched.'
        else if (recovery.document != null)
          '${recovery.document!.label} revision ${recovery.revisionOrCount} was answered before Kineto '
              'closed and has been kept.'
        else if (recovery.images != null)
          '${recovery.revisionOrCount} ${recovery.images!.label} answered before Kineto '
              'closed have been kept.'
        else if (recovery.completed)
          'Generation ${recovery.revisionOrCount} of ${_shotLabel(recovery.shotIndex!)} was interrupted '
              'and has been completed.'
        else
          'Generation ${recovery.revisionOrCount} of ${_shotLabel(recovery.shotIndex!)} was interrupted '
              'and could not be completed (${recovery.error!.explanation}). Kineto tries again the next '
              'time the project opens.',
    ];

/// What shot [shotId] of scene [sceneId] is: a bootstrap shot's demo
/// content, or a planned shot named by its IDs.
DemoShotContent shotContent(String sceneId, String shotId, [KinetoPlannedShot? planned]) {
  if (planned != null) {
    return DemoShotContent(
      label: shotId,
      setting: '${sceneId.replaceAll('_', ' ')} · ${planned.framing.label} · ${planned.lens} · '
          '${planned.cameraMotion.label}',
      description: '${planned.purpose} ${planned.action}',
      candidates: demoShots.first.candidates,
    );
  }
  final number = sceneId == 'scene_001' && shotId.startsWith('shot_')
      ? int.tryParse(shotId.substring('shot_'.length))
      : null;
  if (number != null && number >= 1 && number <= demoShots.length) {
    return demoShots[number - 1];
  }
  return DemoShotContent(
    label: shotId,
    setting: sceneId,
    description: '',
    candidates: demoShots.first.candidates,
  );
}

String _shotLabel(int shotIndex) =>
    shotIndex < demoShots.length ? demoShots[shotIndex].label : 'Shot ${shotIndex + 1}';

class DemoWorkspaceScreen extends StatefulWidget {
  const DemoWorkspaceScreen({
    required this.engine,
    required this.project,
    this.navigation,
    this.isDarkMode = true,
    this.onThemeToggle,
    super.key,
  });

  final KinetoEngine engine;
  final KinetoProjectSession project;

  /// Switches to the other workspaces, shown above the scene.
  final Widget? navigation;
  final bool isDarkMode;
  final VoidCallback? onThemeToggle;

  @override
  State<DemoWorkspaceScreen> createState() => _DemoWorkspaceScreenState();
}

class _DemoWorkspaceScreenState extends State<DemoWorkspaceScreen> {
  late final KinetoEngineSnapshot _engineSnapshot;
  late final KinetoImageProvider _imageProvider;
  late final KinetoVideoProvider _videoProvider;
  late List<KinetoShotSnapshot> _shotSnapshots;
  int _activeShotIndex = 0;
  bool _working = false;
  String _workingLabel = 'Working';
  String? _error;

  /// Generates although a character would be shown without an identity
  /// reference; offered only after the project's policy refused that.
  VoidCallback? _identityDriftConsent;
  late List<String> _recoveryNotes;
  StreamSubscription<KinetoVideoEvent>? _videoEvents;

  /// Takes that ended without a video, by shot: kept until the director
  /// asks for that shot's video again, whatever else they do meanwhile.
  final Map<int, List<String>> _videoFailures = <int, List<String>>{};

  @override
  void initState() {
    super.initState();
    _engineSnapshot = widget.engine.snapshot;
    _imageProvider = widget.project.imageProvider;
    _videoProvider = widget.project.videoProvider;
    _shotSnapshots = _readShotSnapshots();
    _recoveryNotes = describeRecoveries(widget.project.recoveries);
    // Videos finish in the background, whatever the director is doing.
    _videoEvents = widget.project.videoEvents.listen((event) {
      if (!mounted) return;
      setState(() {
        _shotSnapshots = _readShotSnapshots();
        final error = event.error;
        if (error != null) {
          (_videoFailures[event.shotIndex] ??= <String>[]).add(error.explanation);
        }
      });
    });
  }

  @override
  void dispose() {
    unawaited(_videoEvents?.cancel());
    super.dispose();
  }

  List<KinetoShotSnapshot> _readShotSnapshots() =>
      List<KinetoShotSnapshot>.generate(
        widget.project.shotCount,
        widget.project.shotSnapshot,
        growable: false,
      );

  List<String?> _candidateMedia(int shotIndex) => <String?>[
        for (var candidate = 0;
            candidate < _shotSnapshots[shotIndex].candidateCount;
            candidate++)
          widget.project.candidateMediaPath(shotIndex, candidate),
      ];

  List<KinetoCandidateOrigin> _candidateOrigins(int shotIndex) =>
      <KinetoCandidateOrigin>[
        for (var candidate = 0;
            candidate < _shotSnapshots[shotIndex].candidateCount;
            candidate++)
          widget.project.candidateOrigin(shotIndex, candidate),
      ];

  void _switchShot(int index) {
    setState(() {
      _activeShotIndex = index;
      _error = null;
    });
  }

  // Native work runs on the project's worker thread; this isolate only waits
  // for the answer. One command at a time keeps the UI honest about which
  // state the director is looking at.
  //
  // [consenting] generates despite identity drift; it is offered when
  // [action] is refused because the project's policy blocks that.
  Future<void> _run(
    Future<void> Function() action, {
    String label = 'Working',
    Future<void> Function()? consenting,
  }) async {
    if (_working) return;
    setState(() {
      _working = true;
      _workingLabel = label;
      _error = null;
      _identityDriftConsent = null;
    });
    String? error;
    VoidCallback? consent;
    try {
      await action();
    } on KinetoProjectException catch (failure) {
      error = failure.toString();
      if (failure.code == KinetoProjectException.identityDrift && consenting != null) {
        consent = () => _run(consenting, label: label);
      }
    } on Object catch (failure) {
      error = failure.toString();
    }
    if (!mounted) return;
    setState(() {
      _working = false;
      _shotSnapshots = _readShotSnapshots();
      _error = error;
      _identityDriftConsent = consent;
    });
  }

  /// What each shot in play is: a bootstrap shot shows its demo content, a
  /// planned shot its own IDs.
  List<DemoShotContent> _shots() {
    final planning = widget.project.planning;
    return <DemoShotContent>[
      for (var index = 0; index < _shotSnapshots.length; index++)
        shotContent(
          widget.project.shotSceneId(index),
          widget.project.shotId(index),
          planning.shot(widget.project.shotId(index)),
        ),
    ];
  }

  @override
  Widget build(BuildContext context) {
    final snapshot = _shotSnapshots[_activeShotIndex];
    return DemoWorkspaceView(
      shots: _shots(),
      takes: widget.project.takes(_activeShotIndex),
      staleReasons: widget.project.staleReasons(_activeShotIndex),
      onRefine: (note) => _run(
        () => widget.project.refineShot(_activeShotIndex, note),
        label: 'Refining',
        consenting: () => widget.project.refineShot(_activeShotIndex, note, identityDrift: true),
      ),
      engineSnapshot: _engineSnapshot,
      imageProvider: _imageProvider,
      videoProvider: _videoProvider,
      candidateMedia: _candidateMedia(_activeShotIndex),
      candidateOrigins: _candidateOrigins(_activeShotIndex),
      shotSnapshot: snapshot,
      shotSnapshots: _shotSnapshots,
      activeShotIndex: _activeShotIndex,
      isDarkMode: widget.isDarkMode,
      navigation: widget.navigation,
      working: _working,
      workingLabel: _workingLabel,
      recoveryNotes: _recoveryNotes,
      onDismissRecovery: () => setState(() => _recoveryNotes = const <String>[]),
      error: _error,
      onThemeToggle: widget.onThemeToggle,
      onShotChanged: _switchShot,
      onDirectionChanged: (direction) => _run(
        () => widget.project.setShotDirection(_activeShotIndex, direction),
      ),
      // With a plan locked, every shot in play is one of its shots.
      directionPlanned: widget.project.planning.lockedRevision != null,
      keyframesUnresolved: widget.project.keyframesUnresolved(_activeShotIndex),
      onGenerate: () => _run(
        () => widget.project.generateShotCandidates(_activeShotIndex),
        label: 'Generating',
        consenting: () => widget.project.generateShotCandidatesConsenting(
          _activeShotIndex,
          identityDrift: true,
        ),
      ),
      onGenerateDespiteIdentityDrift: _identityDriftConsent,
      onResubmitKeyframes: () => _run(
        () => widget.project.resubmitShotKeyframes(_activeShotIndex),
        label: 'Generating',
      ),
      onSelect: (candidateIndex) => _run(
        () => widget.project.selectShotCandidate(
          _activeShotIndex,
          candidateIndex,
        ),
      ),
      onLock: () => _run(
        () => widget.project.lockShotSelection(_activeShotIndex),
      ),
      onReset: () => _run(
        () => widget.project.resetShot(_activeShotIndex),
      ),
      onMigrate: () => _run(
        () => widget.project.migrateLegacyHashes(_activeShotIndex),
      ),
      videoMedia: widget.project.videoMediaPath(_activeShotIndex),
      videoTakes: widget.project.videoTakes(_activeShotIndex),
      videoEstimate: widget.project.videoEstimate(_activeShotIndex),
      videoFailures: _videoFailures[_activeShotIndex] ?? const <String>[],
      onGenerateVideos: (count, note, branch) => _run(
        () {
          _videoFailures.remove(_activeShotIndex);
          return widget.project.generateShotVideos(
          _activeShotIndex,
          count: count,
          note: note,
          branchFrom: branch,
          );
        },
        label: 'Asking for the video',
      ),
      onSelectVideo: (video) => _run(() => widget.project.selectShotVideo(_activeShotIndex, video)),
      onGenerateVideo: () => _run(
        () => widget.project.generateShotVideo(_activeShotIndex),
        label: 'Submitting video',
      ),
      onResubmitVideo: () => _run(
        () => widget.project.resubmitShotVideo(_activeShotIndex),
        label: 'Submitting video',
      ),
      onLockVideo: () => _run(
        () => widget.project.lockShotVideo(_activeShotIndex),
      ),
      onOpenVideo: openInSystemPlayer,
    );
  }
}

class DemoWorkspaceView extends StatelessWidget {
  const DemoWorkspaceView({
    required this.engineSnapshot,
    required this.shotSnapshot,
    required this.shotSnapshots,
    required this.activeShotIndex,
    required this.onShotChanged,
    required this.onDirectionChanged,
    this.directionPlanned = false,
    required this.onGenerate,
    required this.onSelect,
    required this.onLock,
    required this.onReset,
    this.onMigrate,
    this.keyframesUnresolved = false,
    this.onResubmitKeyframes,
    this.onGenerateDespiteIdentityDrift,
    this.shots = demoShots,
    this.takes = const <KinetoTake>[],
    this.staleReasons = const <List<String>>[],
    this.onRefine,
    this.imageProvider = const KinetoImageProvider(
      kind: KinetoImageProviderKind.localDemo,
      ready: true,
      generationCostMicros: 0,
    ),
    this.videoProvider = const KinetoVideoProvider(
      kind: KinetoVideoProviderKind.localDemo,
      ready: true,
      clipCostMicros: 0,
      clipSeconds: 2,
    ),
    this.candidateMedia = const <String?>[],
    this.videoMedia,
    this.videoTakes = const <KinetoVideoTake>[],
    this.videoEstimate,
    this.videoFailures = const <String>[],
    this.onGenerateVideos,
    this.onSelectVideo,
    this.onGenerateVideo,
    this.onResubmitVideo,
    this.onLockVideo,
    this.onOpenVideo,
    this.candidateOrigins = const <KinetoCandidateOrigin>[],
    this.working = false,
    this.workingLabel = 'Working',
    this.recoveryNotes = const <String>[],
    this.onDismissRecovery,
    this.isDarkMode = true,
    this.onThemeToggle,
    this.error,
    this.navigation,
    super.key,
  });

  /// Switches to the other workspaces, shown above the scene.
  final Widget? navigation;
  final KinetoEngineSnapshot engineSnapshot;
  final KinetoShotSnapshot shotSnapshot;
  final List<KinetoShotSnapshot> shotSnapshots;
  final int activeShotIndex;
  final bool isDarkMode;
  final VoidCallback? onThemeToggle;
  final ValueChanged<int> onShotChanged;
  final ValueChanged<KinetoShotDirection> onDirectionChanged;

  /// The active shot is one of the locked plan's, which gives its direction;
  /// planning its scene again is how that changes.
  final bool directionPlanned;
  final VoidCallback onGenerate;
  final ValueChanged<int> onSelect;
  final VoidCallback onLock;
  final VoidCallback onReset;
  final VoidCallback? onMigrate;

  /// A paid keyframe call of the last generation may have been accepted, and
  /// charged, without its result being kept; generating waits for the
  /// director's explicit decision to risk paying twice.
  final bool keyframesUnresolved;
  final VoidCallback? onResubmitKeyframes;

  /// Generates although a character would be shown without an identity
  /// reference; set only after the project's policy refused that.
  final VoidCallback? onGenerateDespiteIdentityDrift;

  /// What each shot in play is, in order: the bootstrap scene's demo shots,
  /// or the shots of the locked plan.
  final List<DemoShotContent> shots;

  /// Every take the active shot has had, oldest generation first.
  final List<KinetoTake> takes;

  /// Why each current candidate of the active shot is stale.
  final List<List<String>> staleReasons;

  /// Generate the active shot again as the director's note asks.
  final ValueChanged<String>? onRefine;

  /// Who generates candidates for this session, and what one generation costs.
  final KinetoImageProvider imageProvider;

  /// Who makes this session's videos, and what one clip costs.
  final KinetoVideoProvider videoProvider;

  /// Absolute media path of each candidate of the active shot; null for a
  /// candidate without media.
  final List<String?> candidateMedia;

  /// Absolute path of the active shot's current video, if it has one.
  final String? videoMedia;

  /// Every video the active shot has had, oldest generation first.
  final List<KinetoVideoTake> videoTakes;

  /// What one take of the active shot's next video costs, at the clip
  /// length its plan asks for; null to price the provider's shortest clip.
  final KinetoVideoEstimate? videoEstimate;

  /// Why takes of the active shot ended without a video, since it last
  /// asked for one.
  final List<String> videoFailures;

  /// Ask for a video generation: how many takes, a note, a take to branch
  /// from.
  final void Function(int count, String? note, String? branchFrom)? onGenerateVideos;
  final ValueChanged<int>? onSelectVideo;
  final VoidCallback? onGenerateVideo;

  /// Submit the video again after an unresolved submission, risking a second
  /// charge.
  final VoidCallback? onResubmitVideo;
  final VoidCallback? onLockVideo;
  final ValueChanged<String>? onOpenVideo;
  /// Who made each candidate of the active shot, as its artifact records it.
  /// Candidate cards name this, never [imageProvider]: a project reopened
  /// with another provider still shows who made its existing candidates.
  final List<KinetoCandidateOrigin> candidateOrigins;

  /// A command is running on the project's worker; changes wait for it.
  final bool working;

  /// What the running command is, for example 'Generating'.
  final String workingLabel;

  /// What the last open recovered from an interrupted session.
  final List<String> recoveryNotes;
  final VoidCallback? onDismissRecovery;
  final String? error;

  @override
  Widget build(BuildContext context) {
    final selectedIndex = shotSnapshot.selectedIndex;
    final blocked = shotSnapshot.changesBlocked || working;
    final canGenerate = !blocked && imageProvider.ready && !keyframesUnresolved;
    // A take made in a style or cast since moved on from is not locked,
    // even when the shot's own intent is unchanged.
    final selectedStale = shotSnapshot.stale ||
        (selectedIndex != null &&
            selectedIndex < staleReasons.length &&
            staleReasons[selectedIndex].isNotEmpty);
    final canLock = !blocked &&
        shotSnapshot.generated &&
        selectedIndex != null &&
        !shotSnapshot.locked &&
        !selectedStale;
    final shot = shots[activeShotIndex];
    final sceneReady = shotSnapshots.every((snapshot) => snapshot.locked);

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
                WabPanel(
                  title: 'Canonical demo project',
                  trailing: Text('Native ABI ${engineSnapshot.abiVersion}'),
                  child: Text(
                    imageProvider.isPaid
                        ? 'Keyframe candidates come from ${imageProvider.kind.label}, compiled from each '
                            "shot's written intent. Every generation is a paid call; choose, lock, then "
                            'reopen Kineto to verify canonical project state.'
                        : 'A deterministic local vertical slice. Work through two independent shots, choose direction, '
                            'generate candidates, select and lock, then reopen Kineto to verify canonical project state.',
                  ),
                ),
                if (recoveryNotes.isNotEmpty) ...<Widget>[
                  const SizedBox(height: 20),
                  WabPanel(
                    title: 'Recovered interrupted work',
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: <Widget>[
                        for (final note in recoveryNotes) ...<Widget>[
                          Text(note),
                          const SizedBox(height: 8),
                        ],
                        const SizedBox(height: 4),
                        WabButton(
                          kind: WabMaterialKind.zhuwen,
                          onPressed: onDismissRecovery,
                          child: const Text('Dismiss'),
                        ),
                      ],
                    ),
                  ),
                ],
                const SizedBox(height: 20),
                WabPanel(
                  title: 'Scene 001',
                  trailing: working
                      ? WabStatusBadge(workingLabel, kind: WabBadgeKind.progress)
                      : WabStatusBadge(
                          sceneReady ? 'Ready' : 'In progress',
                          kind: sceneReady ? WabBadgeKind.done : WabBadgeKind.progress,
                        ),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: <Widget>[
                      const Text(
                        'Two production beats share the scene but keep independent approval state.',
                      ),
                      const SizedBox(height: 12),
                      Wrap(
                        spacing: 16,
                        runSpacing: 12,
                        children: <Widget>[
                          for (var index = 0; index < shots.length; index++)
                            Row(
                              mainAxisSize: MainAxisSize.min,
                              children: <Widget>[
                                WabButton(
                                  kind: index == activeShotIndex
                                      ? WabMaterialKind.seal
                                      : WabMaterialKind.zhuwen,
                                  onPressed: () => onShotChanged(index),
                                  child: Text(shots[index].label),
                                ),
                                const SizedBox(width: 8),
                                WabStatusBadge(
                                  _shotStatusLabel(shotSnapshots[index]),
                                  kind: _shotStatusKind(shotSnapshots[index]),
                                ),
                              ],
                            ),
                        ],
                      ),
                    ],
                  ),
                ),
                const SizedBox(height: 20),
                if (shotSnapshot.hasInvalidHashes) ...<Widget>[
                  WabPanel(
                    title: 'Unrecognised stored hashes',
                    child: Text(
                      'This shot contains hashes this build cannot verify. It stays readable; changes are '
                      'refused until the records are repaired.'
                      '${shotSnapshot.migrationPending ? ' Legacy hash migration is refused until then too.' : ''}',
                    ),
                  ),
                  const SizedBox(height: 20),
                ] else if (shotSnapshot.canMigrate) ...<Widget>[
                  WabPanel(
                    title: 'Legacy hashes need migration',
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: <Widget>[
                        const Text(
                          'This shot was saved by an older Kineto build. It stays readable, but changes are '
                          'refused until its hashes are rewritten to the current form. The original records '
                          'are kept in artifacts.pre-sha256-migration.json.',
                        ),
                        const SizedBox(height: 12),
                        WabButton(
                          kind: WabMaterialKind.seal,
                          onPressed: working ? null : onMigrate,
                          child: const Text('Migrate with backup'),
                        ),
                      ],
                    ),
                  ),
                  const SizedBox(height: 20),
                ],
                WabPanel(
                  title: 'Shot direction',
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: <Widget>[
                      Text(
                        shotSnapshot.locked
                            ? "Direction is frozen with this shot's locked selection."
                            : directionPlanned
                                ? "This shot's direction comes from the locked shot plan. To change it, plan "
                                    'the scene again in Shot plan and say which direction you want.'
                                : 'Changing direction preserves generated artifacts for provenance and marks them stale until you regenerate.',
                      ),
                      const SizedBox(height: 12),
                      Wrap(
                        spacing: 12,
                        runSpacing: 12,
                        children: <Widget>[
                          for (final direction in KinetoShotDirection.values)
                            WabButton(
                              key: ValueKey<String>('direction-${direction.wireName}'),
                              kind: direction == shotSnapshot.direction
                                  ? WabMaterialKind.seal
                                  : WabMaterialKind.zhuwen,
                              onPressed: shotSnapshot.locked || directionPlanned || blocked
                                  ? null
                                  : () => onDirectionChanged(direction),
                              child: Text(direction.label),
                            ),
                        ],
                      ),
                    ],
                  ),
                ),
                const SizedBox(height: 20),
                WabPanel(
                  title: 'Scene 001 · ${shot.label}',
                  trailing: Text(shotSnapshot.direction.label),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: <Widget>[
                      Text(
                        shot.setting,
                        style: Theme.of(context).textTheme.titleMedium,
                      ),
                      const SizedBox(height: 8),
                      Text(shot.description),
                      const SizedBox(height: 16),
                      if (!imageProvider.ready) ...<Widget>[
                        Text(
                          '${imageProvider.kind.label} is selected but has no API key; set '
                          'KINETO_${imageProvider.kind.wireName.toUpperCase()}_API_KEY and reopen Kineto.',
                        ),
                        const SizedBox(height: 12),
                      ],
                      if (keyframesUnresolved) ...<Widget>[
                        const Text(
                          'The last generation did not finish cleanly, and the image provider may already '
                          'have charged for keyframes Kineto never received. Check your account with it; '
                          'generate again only if you accept paying twice.',
                        ),
                        const SizedBox(height: 12),
                        WabButton(
                          kind: WabMaterialKind.zhuwen,
                          onPressed: blocked || !imageProvider.ready ? null : onResubmitKeyframes,
                          child: Text(
                            'Generate again · may pay twice · ≈ ${imageProvider.generationCostLabel}',
                          ),
                        ),
                        const SizedBox(height: 12),
                      ],
                      if (!shotSnapshot.generated)
                        WabElevatedButton(
                          text: Text(_generateLabel(imageProvider)),
                          callback: canGenerate ? onGenerate : null,
                        )
                      else
                        Row(
                          children: <Widget>[
                            Expanded(child: Text(_generationStatus(shotSnapshot))),
                            const SizedBox(width: 16),
                            WabButton(
                              kind: WabMaterialKind.zhuwen,
                              onPressed:
                                  shotSnapshot.locked || !canGenerate ? null : onGenerate,
                              child: Text(
                                imageProvider.isPaid
                                    ? 'Regenerate · ≈ ${imageProvider.generationCostLabel}'
                                    : 'Regenerate',
                              ),
                            ),
                            if (onRefine != null) ...<Widget>[
                              const SizedBox(width: 12),
                              WabButton(
                                key: const ValueKey<String>('refine-shot'),
                                kind: WabMaterialKind.paper,
                                onPressed: shotSnapshot.locked || !canGenerate
                                    ? null
                                    : () => _refineDialog(context),
                                child: const Text('Refine with a note'),
                              ),
                            ],
                          ],
                        ),
                    ],
                  ),
                ),
                if (shotSnapshot.generated) ...<Widget>[
                  const SizedBox(height: 20),
                  WabPanel(
                    title: 'Generation ${shotSnapshot.generation} · Candidates',
                    child: Wrap(
                      spacing: 16,
                      runSpacing: 16,
                      children: List<Widget>.generate(
                        shotSnapshot.candidateCount,
                        (index) {
                          final selected = selectedIndex == index;
                          final media =
                              index < candidateMedia.length ? candidateMedia[index] : null;
                          final origin =
                              index < candidateOrigins.length ? candidateOrigins[index].label : null;
                          final buttonLabel = shotSnapshot.locked
                              ? (selected ? 'Locked' : 'Candidate')
                              : (selected ? 'Selected' : 'Select');
                          final onPressed =
                              shotSnapshot.locked || blocked ? null : () => onSelect(index);
                          if (media == null) {
                            final content = shot.candidates[index];
                            return SizedBox(
                              width: 300,
                              child: WabCollectionCard(
                                icon: Icon(content.icon, size: 34),
                                title: content.title,
                                description: content.description,
                                buttonLabel: buttonLabel,
                                highlighted: selected,
                                onPressed: onPressed,
                              ),
                            );
                          }
                          return SizedBox(
                            width: 300,
                            child: Column(
                              crossAxisAlignment: CrossAxisAlignment.stretch,
                              children: <Widget>[
                                _Keyframe(path: media),
                                const SizedBox(height: 8),
                                WabCollectionCard(
                                  icon: const Icon(Icons.photo_camera_back_outlined, size: 30),
                                  title: 'Take ${String.fromCharCode(0x41 + index)}',
                                  description: origin == null
                                      ? 'Keyframe · ${_fileName(media)}'
                                      : '$origin keyframe · ${_fileName(media)}',
                                  buttonLabel: buttonLabel,
                                  highlighted: selected,
                                  onPressed: onPressed,
                                ),
                              ],
                            ),
                          );
                        },
                      ),
                    ),
                  ),
                  for (var index = 0; index < staleReasons.length; index++)
                    if (staleReasons[index].isNotEmpty)
                      Padding(
                        padding: const EdgeInsets.only(top: 8),
                        child: Text(
                          'Take ${String.fromCharCode(0x41 + index)} no longer follows: '
                          '${staleReasons[index].join(', ')}',
                        ),
                      ),
                  const SizedBox(height: 20),
                  Row(
                    children: <Widget>[
                      Expanded(
                        child: WabButton(
                          key: const ValueKey<String>('lock-selection'),
                          kind: WabMaterialKind.seal,
                          onPressed: canLock ? onLock : null,
                          expand: true,
                          child: Text(
                            shotSnapshot.locked
                                ? 'Selection locked'
                                : selectedStale
                                    ? 'Regenerate before locking'
                                    : 'Lock selection',
                          ),
                        ),
                      ),
                      const SizedBox(width: 16),
                      Expanded(
                        child: WabButton(
                          kind: WabMaterialKind.zhuwen,
                          onPressed: blocked ? null : onReset,
                          expand: true,
                          child: const Text('Reset shot'),
                        ),
                      ),
                    ],
                  ),
                  if (takes.any((take) => take.status == KinetoTakeStatus.superseded)) ...<Widget>[
                    const SizedBox(height: 20),
                    _EarlierTakes(takes: takes),
                  ],
                ],
                if (shotSnapshot.locked) ...<Widget>[
                  const SizedBox(height: 20),
                  _VideoPanel(
                    // A shot's choices are its own: the take count starts
                    // over on every shot.
                    key: ValueKey<String>('video-panel-${shots[activeShotIndex].label}'),
                    snapshot: shotSnapshot,
                    provider: videoProvider,
                    estimate: videoEstimate,
                    failures: videoFailures,
                    poster: selectedIndex != null && selectedIndex < candidateMedia.length
                        ? candidateMedia[selectedIndex]
                        : null,
                    takes: videoTakes,
                    blocked: blocked,
                    onGenerate: onGenerateVideos ??
                        (onGenerateVideo == null ? null : (count, note, branch) => onGenerateVideo!()),
                    onResubmit: onResubmitVideo,
                    onSelect: onSelectVideo,
                    onLock: onLockVideo,
                    onOpen: onOpenVideo,
                  ),
                ],
                if (error != null) ...<Widget>[
                  const SizedBox(height: 20),
                  WabPanel(
                    title: 'Native operation failed',
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: <Widget>[
                        Text(error!),
                        if (onGenerateDespiteIdentityDrift != null) ...<Widget>[
                          const SizedBox(height: 12),
                          const Text(
                            "This project keeps every character's identity from their locked cast. "
                            'Generate anyway only if a different-looking person is acceptable for '
                            'these keyframes.',
                          ),
                          const SizedBox(height: 12),
                          WabButton(
                            key: const ValueKey<String>('generate-despite-drift'),
                            kind: WabMaterialKind.zhuwen,
                            onPressed: blocked ? null : onGenerateDespiteIdentityDrift,
                            child: const Text('Generate anyway'),
                          ),
                        ],
                      ],
                    ),
                  ),
                ],
              ],
            ),
          ),
        ),
      ),
    );
  }

  Future<void> _refineDialog(BuildContext context) async {
    final note = await showDialog<String>(
      context: context,
      builder: (context) => TextPromptDialog(
        title: 'Refine ${shots[activeShotIndex].label}',
        width: 560,
        fieldKey: 'refine-shot-note',
        minLines: 2,
        maxLines: 5,
        hint: 'What should change, such as "warmer light, tighter on his hands"',
        confirmKey: 'refine-shot-confirm',
        confirmLabel: imageProvider.isPaid
            ? 'Generate · ≈ ${imageProvider.generationCostLabel}'
            : 'Generate',
      ),
    );
    if (note == null || note.trim().isEmpty) return;
    onRefine?.call(note.trim());
  }

  static String _generateLabel(KinetoImageProvider provider) => provider.isPaid
      ? 'Generate 3 keyframes · ${provider.kind.label} · ≈ ${provider.generationCostLabel}'
      : 'Generate 3 deterministic candidates';

  static String _fileName(String path) => path.split(Platform.pathSeparator).last;

  static String _generationStatus(KinetoShotSnapshot snapshot) {
    if (snapshot.locked) {
      return 'Generation ${snapshot.generation} locked · reopen the app to verify canonical persistence.';
    }
    if (snapshot.stale) {
      return 'Generation ${snapshot.generation} is stale after the direction changed · regenerate before locking.';
    }
    return 'Generation ${snapshot.generation} · ${snapshot.supersededCount} superseded · choose the shot you want to keep.';
  }

  static String _shotStatusLabel(KinetoShotSnapshot snapshot) {
    if (snapshot.locked) return 'Locked';
    if (snapshot.stale) return 'Stale';
    if (snapshot.selectedIndex != null) return 'Selected';
    if (snapshot.generated) return 'Candidates';
    return 'Empty';
  }

  static WabBadgeKind _shotStatusKind(KinetoShotSnapshot snapshot) {
    if (snapshot.locked) return WabBadgeKind.done;
    if (snapshot.generated) return WabBadgeKind.progress;
    return WabBadgeKind.neutral;
  }
}

/// Takes of earlier generations, kept to compare with the current ones.
class _EarlierTakes extends StatelessWidget {
  const _EarlierTakes({required this.takes});

  final List<KinetoTake> takes;

  @override
  Widget build(BuildContext context) => WabPanel(
        title: 'Earlier takes',
        child: Wrap(
          spacing: 12,
          runSpacing: 12,
          children: <Widget>[
            for (final take in takes.where((take) => take.status == KinetoTakeStatus.superseded))
              SizedBox(
                width: 200,
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: <Widget>[
                    if (take.mediaPath != null) _Keyframe(path: take.mediaPath!),
                    const SizedBox(height: 4),
                    Text(
                      'Generation ${take.generation}${take.refined ? ' · refined' : ''} · '
                      '${take.id.split('_').last}',
                    ),
                  ],
                ),
              ),
          ],
        ),
      );
}

/// One keyframe candidate, as a 16:9 still read from the project directory.
class _Keyframe extends StatelessWidget {
  const _Keyframe({required this.path});

  final String path;

  @override
  Widget build(BuildContext context) => ClipRRect(
        borderRadius: BorderRadius.circular(2),
        child: AspectRatio(
          aspectRatio: 16 / 9,
          child: Image.file(
            File(path),
            fit: BoxFit.cover,
            // A keyframe that cannot be read says so instead of vanishing.
            errorBuilder: (context, error, stackTrace) => const ColoredBox(
              color: Colors.black26,
              child: Center(child: Text('Keyframe unavailable')),
            ),
          ),
        ),
      );
}

/// Open a video in the platform's default player. Kineto has no video
/// playback of its own yet.
Future<void> openInSystemPlayer(String path) async {
  final (executable, arguments) = Platform.isMacOS
      ? ('open', <String>[path])
      : Platform.isWindows
          ? ('cmd', <String>['/c', 'start', '', path])
          : ('xdg-open', <String>[path]);
  await Process.start(executable, arguments, mode: ProcessStartMode.detached);
}

/// The shot's video, once its keyframe is locked: the latest generation's
/// takes to compare and choose from, the earlier ones to branch from, and
/// what a new generation costs.
class _VideoPanel extends StatefulWidget {
  const _VideoPanel({
    required this.snapshot,
    this.estimate,
    this.failures = const <String>[],
    super.key,
    required this.provider,
    required this.poster,
    required this.takes,
    required this.blocked,
    this.onGenerate,
    this.onResubmit,
    this.onSelect,
    this.onLock,
    this.onOpen,
  });

  final KinetoShotSnapshot snapshot;
  final KinetoVideoProvider provider;
  final KinetoVideoEstimate? estimate;

  /// Why takes ended without a video.
  final List<String> failures;
  final String? poster;
  final List<KinetoVideoTake> takes;
  final bool blocked;
  final void Function(int count, String? note, String? branchFrom)? onGenerate;
  final VoidCallback? onResubmit;
  final ValueChanged<int>? onSelect;
  final VoidCallback? onLock;
  final ValueChanged<String>? onOpen;

  @override
  State<_VideoPanel> createState() => _VideoPanelState();
}

class _VideoPanelState extends State<_VideoPanel> {
  int _count = 1;

  KinetoVideoProvider get _provider => widget.provider;

  /// The clip length this shot's takes are made at.
  int get _clipSeconds => widget.estimate?.clipSeconds ?? _provider.clipSeconds;

  /// What [count] takes of this shot cost.
  String _costFor(int count) => widget.estimate?.costFor(count) ?? _provider.costFor(count);

  @override
  Widget build(BuildContext context) {
    final state = widget.snapshot.videoState;
    final current = widget.takes.where((take) => take.current).toList();
    final earlier = widget.takes.where((take) => !take.current).toList().reversed.toList();
    final locked = current.any((take) => take.status == KinetoTakeStatus.locked);
    final hasFirstFrame = widget.poster != null || !_provider.needsFrameImage;
    final canGenerate = _provider.ready &&
        hasFirstFrame &&
        !widget.blocked &&
        !locked &&
        state != KinetoVideoState.generating &&
        state != KinetoVideoState.unresolved;
    final chosen = current.where((take) => take.status == KinetoTakeStatus.selected).firstOrNull ??
        (current.length == 1 ? current.single : null);
    final canLock = !widget.blocked &&
        !locked &&
        state != KinetoVideoState.generating &&
        chosen != null &&
        chosen.stale.isEmpty &&
        chosen.mediaPath != null;
    final takes = _count == 1 ? '1 take' : '$_count takes';
    return WabPanel(
      title: 'Video',
      trailing: WabStatusBadge(
        _label(state),
        kind: switch (state) {
          KinetoVideoState.locked => WabBadgeKind.done,
          KinetoVideoState.none => WabBadgeKind.neutral,
          KinetoVideoState.unresolved || KinetoVideoState.stale => WabBadgeKind.warning,
          _ => WabBadgeKind.progress,
        },
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Text(_explanation(state)),
          for (final (index, failure) in widget.failures.indexed)
            Padding(
              key: ValueKey<String>('video-failure-$index'),
              padding: const EdgeInsets.only(top: 8),
              child: Text('A take did not arrive: $failure.'),
            ),
          if (current.isNotEmpty) ...<Widget>[
            const SizedBox(height: 12),
            Wrap(
              spacing: 12,
              runSpacing: 12,
              children: <Widget>[
                for (final (index, take) in current.indexed) _take(take, index, current.length, locked),
              ],
            ),
          ],
          const SizedBox(height: 12),
          Wrap(
            spacing: 12,
            runSpacing: 12,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: <Widget>[
              if (canLock)
                WabButton(
                  key: const ValueKey<String>('lock-video'),
                  kind: WabMaterialKind.seal,
                  onPressed: widget.onLock,
                  child: Text(chosen.letter.isEmpty ? 'Lock video' : 'Lock take ${chosen.letter}'),
                ),
              if (_provider.ready && state == KinetoVideoState.unresolved)
                WabButton(
                  kind: WabMaterialKind.zhuwen,
                  onPressed: widget.blocked || !hasFirstFrame ? null : widget.onResubmit,
                  child: Text('Submit again · may pay twice · ${_costFor(1)}'),
                ),
              if (_provider.ready && !locked && state != KinetoVideoState.unresolved) ...<Widget>[
                SizedBox(
                  width: 160,
                  child: WabDropdown<int>(
                    key: const ValueKey<String>('video-count'),
                    value: _count,
                    items: <int, String>{
                      for (var count = 1; count <= kinetoMaxVideoCandidates; count++)
                        count: count == 1 ? '1 take' : '$count takes',
                    },
                    onChanged: (count) => setState(() => _count = count ?? _count),
                  ),
                ),
                WabButton(
                  key: const ValueKey<String>('generate-videos'),
                  kind: WabMaterialKind.zhuwen,
                  onPressed: canGenerate ? () => widget.onGenerate?.call(_count, null, null) : null,
                  child: Text(
                    '${current.isEmpty ? 'Generate' : 'Generate again'} · $takes · '
                    '$_clipSeconds s · ${_costFor(_count)}',
                  ),
                ),
                if (current.isNotEmpty)
                  WabButton(
                    key: const ValueKey<String>('refine-video'),
                    kind: WabMaterialKind.paper,
                    onPressed: canGenerate ? () => _ask(context, null) : null,
                    child: const Text('Refine with a note…'),
                  ),
              ],
            ],
          ),
          if (earlier.isNotEmpty) ...<Widget>[
            const SizedBox(height: 16),
            Text('Earlier takes', style: Theme.of(context).textTheme.titleSmall),
            const SizedBox(height: 8),
            for (final take in earlier)
              Padding(
                padding: const EdgeInsets.only(bottom: 8),
                child: Wrap(
                  spacing: 12,
                  runSpacing: 8,
                  crossAxisAlignment: WrapCrossAlignment.center,
                  children: <Widget>[
                    Text(_title(take)),
                    if (take.notes.isNotEmpty) Text('Notes: ${take.notes.join('; ')}'),
                    if (take.mediaPath != null && widget.onOpen != null)
                      WabButton(
                        kind: WabMaterialKind.paper,
                        onPressed: () => widget.onOpen!(take.mediaPath!),
                        child: const Text('Open'),
                      )
                    else if (take.mediaPath == null)
                      const Text('Its clip was let go.'),
                    WabButton(
                      key: ValueKey<String>('branch-${take.id}'),
                      kind: WabMaterialKind.paper,
                      onPressed: canGenerate ? () => _ask(context, take) : null,
                      child: const Text('Branch from it…'),
                    ),
                  ],
                ),
              ),
          ],
        ],
      ),
    );
  }

  Widget _take(KinetoVideoTake take, int index, int takes, bool locked) {
    final chosen = take.status == KinetoTakeStatus.selected;
    return SizedBox(
      width: 260,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          if (widget.poster != null)
            _Keyframe(path: widget.poster!)
          else
            const AspectRatio(aspectRatio: 16 / 9, child: ColoredBox(color: Colors.black26)),
          const SizedBox(height: 6),
          Text(
            '${_title(take)}'
            '${take.status == KinetoTakeStatus.locked ? ' · locked' : chosen ? ' · chosen' : ''}',
          ),
          if (take.notes.isNotEmpty) Text('Notes: ${take.notes.join('; ')}'),
          if (take.branchedFrom != null) Text('Branched from ${take.branchedFrom}'),
          if (take.stale.isNotEmpty) Text('No longer follows: ${take.stale.join(', ')}'),
          const SizedBox(height: 6),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: <Widget>[
              if (take.mediaPath != null && widget.onOpen != null)
                WabButton(
                  key: ValueKey<String>('open-${take.id}'),
                  kind: WabMaterialKind.seal,
                  onPressed: () => widget.onOpen!(take.mediaPath!),
                  child: const Text('Open video'),
                ),
              if (!locked && !chosen && takes > 1)
                WabButton(
                  key: ValueKey<String>('choose-${take.id}'),
                  kind: WabMaterialKind.paper,
                  onPressed: widget.blocked ? null : () => widget.onSelect?.call(index),
                  child: const Text('Choose'),
                ),
            ],
          ),
        ],
      ),
    );
  }

  /// Ask for a note, then a new generation with it: a refinement, or a
  /// branch from [from].
  Future<void> _ask(BuildContext context, KinetoVideoTake? from) async {
    final takes = _count == 1 ? '1 take' : '$_count takes';
    final note = await showDialog<String>(
      context: context,
      builder: (context) => TextPromptDialog(
        title: from == null ? 'Refine the video' : 'Branch from ${_title(from)}',
        width: 520,
        fieldKey: 'video-note',
        minLines: 2,
        maxLines: 4,
        hint: from == null
            ? 'What should change, such as "slower, hold on his eyes"'
            : 'What should change from this take; its own notes are kept',
        confirmKey: 'video-note-confirm',
        confirmLabel: 'Generate · $takes · ${_costFor(_count)}',
      ),
    );
    if (note == null) return;
    final trimmed = note.trim();
    if (from == null && trimmed.isEmpty) return;
    widget.onGenerate?.call(_count, trimmed.isEmpty ? null : trimmed, from?.id);
  }

  static String _title(KinetoVideoTake take) =>
      take.letter.isEmpty ? 'Generation ${take.generation}' : 'Generation ${take.generation} · take ${take.letter}';

  static String _label(KinetoVideoState state) => switch (state) {
        KinetoVideoState.none => 'No video',
        KinetoVideoState.generating => 'Generating',
        KinetoVideoState.ready => 'Ready',
        KinetoVideoState.locked => 'Locked',
        KinetoVideoState.stale => 'Stale',
        KinetoVideoState.unresolved => 'Unresolved',
      };

  String _explanation(KinetoVideoState state) {
    if (!_provider.ready) {
      return '${_provider.kind.label} has no key configured, so no video can be asked for.';
    }
    if (widget.poster == null && _provider.needsFrameImage) {
      return 'The locked keyframe has no image to start a video from; regenerate the keyframes '
          'with an image provider.';
    }
    return switch (state) {
      KinetoVideoState.none => _provider.isPaid
          ? 'The locked keyframe becomes the first frame of $_clipSeconds-second clips, '
              'generated in the background. Ask for several takes to compare.'
          : 'The local demo makes $_clipSeconds-second placeholder clips from the locked '
              'keyframe, free and at once.',
      KinetoVideoState.generating =>
        '${_provider.kind.label} is generating the video. It can take a few minutes; you can keep '
            'working, and closing Kineto does not lose it.',
      KinetoVideoState.ready => 'Open the takes, choose one and lock it, refine them with a note, or '
          'branch from an earlier take.',
      KinetoVideoState.locked => 'The video is locked as production truth.',
      KinetoVideoState.stale =>
        'The video no longer follows the shot: its locked keyframe or its planned intent changed '
            'since; generate it again.',
      KinetoVideoState.unresolved =>
        'The last submission did not finish cleanly, and ${_provider.kind.label} may already have '
            'accepted and charged for it. Kineto has no record of its task, so it cannot follow it. '
            'Check your account; submit again only if you accept paying twice.',
    };
  }
}
