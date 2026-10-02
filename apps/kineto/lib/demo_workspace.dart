import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:wabisabi/wabisabi.dart';

import 'native/kineto_engine.dart';

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
        else if (recovery.completed)
          'Generation ${recovery.revisionOrCount} of ${_shotLabel(recovery.shotIndex!)} was interrupted '
              'and has been completed.'
        else
          'Generation ${recovery.revisionOrCount} of ${_shotLabel(recovery.shotIndex!)} was interrupted '
              'and could not be completed (${recovery.error!.message}). Kineto tries again the next '
              'time the project opens.',
    ];

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
  late List<KinetoShotSnapshot> _shotSnapshots;
  int _activeShotIndex = 0;
  bool _working = false;
  String _workingLabel = 'Working';
  String? _error;
  late List<String> _recoveryNotes;
  StreamSubscription<KinetoVideoEvent>? _videoEvents;

  @override
  void initState() {
    super.initState();
    _engineSnapshot = widget.engine.snapshot;
    _imageProvider = widget.project.imageProvider;
    _shotSnapshots = _readShotSnapshots();
    _recoveryNotes = describeRecoveries(widget.project.recoveries);
    // Videos finish in the background, whatever the director is doing.
    _videoEvents = widget.project.videoEvents.listen((event) {
      if (!mounted) return;
      setState(() {
        _shotSnapshots = _readShotSnapshots();
        final error = event.error;
        if (error != null) _error = error.toString();
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
        KinetoProjectSession.shotCount,
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
  Future<void> _run(
    Future<void> Function() action, {
    String label = 'Working',
  }) async {
    if (_working) return;
    setState(() {
      _working = true;
      _workingLabel = label;
      _error = null;
    });
    String? error;
    try {
      await action();
    } on Object catch (failure) {
      error = failure.toString();
    }
    if (!mounted) return;
    setState(() {
      _working = false;
      _shotSnapshots = _readShotSnapshots();
      _error = error;
    });
  }

  @override
  Widget build(BuildContext context) {
    final snapshot = _shotSnapshots[_activeShotIndex];
    return DemoWorkspaceView(
      engineSnapshot: _engineSnapshot,
      imageProvider: _imageProvider,
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
      onGenerate: () => _run(
        () => widget.project.generateShotCandidates(_activeShotIndex),
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
      onGenerateVideo: () => _run(
        () => widget.project.generateShotVideo(_activeShotIndex),
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
    required this.onGenerate,
    required this.onSelect,
    required this.onLock,
    required this.onReset,
    this.onMigrate,
    this.imageProvider = const KinetoImageProvider(
      kind: KinetoImageProviderKind.localDemo,
      ready: true,
      generationCostMicros: 0,
    ),
    this.candidateMedia = const <String?>[],
    this.videoMedia,
    this.onGenerateVideo,
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
  final VoidCallback onGenerate;
  final ValueChanged<int> onSelect;
  final VoidCallback onLock;
  final VoidCallback onReset;
  final VoidCallback? onMigrate;

  /// Who generates candidates for this session, and what one generation costs.
  final KinetoImageProvider imageProvider;

  /// Absolute media path of each candidate of the active shot; null for a
  /// candidate without media.
  final List<String?> candidateMedia;

  /// Absolute path of the active shot's current video, if it has one.
  final String? videoMedia;
  final VoidCallback? onGenerateVideo;
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
    final canGenerate = !blocked && imageProvider.ready;
    final canLock = !blocked &&
        shotSnapshot.generated &&
        selectedIndex != null &&
        !shotSnapshot.locked &&
        !shotSnapshot.stale;
    final shot = demoShots[activeShotIndex];
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
                          for (var index = 0; index < demoShots.length; index++)
                            Row(
                              mainAxisSize: MainAxisSize.min,
                              children: <Widget>[
                                WabButton(
                                  kind: index == activeShotIndex
                                      ? WabMaterialKind.seal
                                      : WabMaterialKind.zhuwen,
                                  onPressed: () => onShotChanged(index),
                                  child: Text(demoShots[index].label),
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
                            : 'Changing direction preserves generated artifacts for provenance and marks them stale until you regenerate.',
                      ),
                      const SizedBox(height: 12),
                      Wrap(
                        spacing: 12,
                        runSpacing: 12,
                        children: <Widget>[
                          for (final direction in KinetoShotDirection.values)
                            WabButton(
                              kind: direction == shotSnapshot.direction
                                  ? WabMaterialKind.seal
                                  : WabMaterialKind.zhuwen,
                              onPressed: shotSnapshot.locked || blocked
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
                  const SizedBox(height: 20),
                  Row(
                    children: <Widget>[
                      Expanded(
                        child: WabButton(
                          kind: WabMaterialKind.seal,
                          onPressed: canLock ? onLock : null,
                          expand: true,
                          child: Text(
                            shotSnapshot.locked
                                ? 'Selection locked'
                                : shotSnapshot.stale
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
                ],
                if (shotSnapshot.locked) ...<Widget>[
                  const SizedBox(height: 20),
                  _VideoPanel(
                    snapshot: shotSnapshot,
                    provider: imageProvider,
                    poster: selectedIndex != null && selectedIndex < candidateMedia.length
                        ? candidateMedia[selectedIndex]
                        : null,
                    video: videoMedia,
                    blocked: blocked,
                    onGenerate: onGenerateVideo,
                    onLock: onLockVideo,
                    onOpen: onOpenVideo,
                  ),
                ],
                if (error != null) ...<Widget>[
                  const SizedBox(height: 20),
                  WabPanel(
                    title: 'Native operation failed',
                    child: Text(error!),
                  ),
                ],
              ],
            ),
          ),
        ),
      ),
    );
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

/// The shot's video, once its keyframe is locked.
class _VideoPanel extends StatelessWidget {
  const _VideoPanel({
    required this.snapshot,
    required this.provider,
    required this.poster,
    required this.video,
    required this.blocked,
    this.onGenerate,
    this.onLock,
    this.onOpen,
  });

  final KinetoShotSnapshot snapshot;
  final KinetoImageProvider provider;
  final String? poster;
  final String? video;
  final bool blocked;
  final VoidCallback? onGenerate;
  final VoidCallback? onLock;
  final ValueChanged<String>? onOpen;

  @override
  Widget build(BuildContext context) {
    final state = snapshot.videoState;
    final canGenerate = provider.canGenerateVideo &&
        poster != null &&
        !blocked &&
        state != KinetoVideoState.generating &&
        state != KinetoVideoState.locked;
    final generateLabel = state == KinetoVideoState.none
        ? 'Generate video · 6 s · ≈ ${provider.videoCostLabel}'
        : 'Regenerate video · ≈ ${provider.videoCostLabel}';
    return WabPanel(
      title: 'Video',
      trailing: WabStatusBadge(
        _label(state),
        kind: switch (state) {
          KinetoVideoState.locked => WabBadgeKind.done,
          KinetoVideoState.none => WabBadgeKind.neutral,
          _ => WabBadgeKind.progress,
        },
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Text(_explanation(state)),
          if (poster != null && video != null) ...<Widget>[
            const SizedBox(height: 12),
            SizedBox(width: 480, child: _Keyframe(path: poster!)),
          ],
          const SizedBox(height: 12),
          Wrap(
            spacing: 12,
            runSpacing: 12,
            children: <Widget>[
              if (video != null && onOpen != null)
                WabButton(
                  kind: WabMaterialKind.seal,
                  onPressed: () => onOpen!(video!),
                  child: const Text('Open video'),
                ),
              if (state == KinetoVideoState.ready)
                WabButton(
                  kind: WabMaterialKind.seal,
                  onPressed: blocked ? null : onLock,
                  child: const Text('Lock video'),
                ),
              if (provider.canGenerateVideo && state != KinetoVideoState.locked)
                WabButton(
                  kind: WabMaterialKind.zhuwen,
                  onPressed: canGenerate ? onGenerate : null,
                  child: Text(generateLabel),
                ),
            ],
          ),
        ],
      ),
    );
  }

  static String _label(KinetoVideoState state) => switch (state) {
        KinetoVideoState.none => 'No video',
        KinetoVideoState.generating => 'Generating',
        KinetoVideoState.ready => 'Ready',
        KinetoVideoState.locked => 'Locked',
        KinetoVideoState.stale => 'Stale',
      };

  String _explanation(KinetoVideoState state) {
    if (!provider.canGenerateVideo) {
      return 'Video starts from a generated keyframe; select MiniMax to generate one.';
    }
    if (poster == null) {
      return 'The locked keyframe has no image to start a video from; regenerate the keyframes.';
    }
    return switch (state) {
      KinetoVideoState.none =>
        'The locked keyframe becomes the first frame of a 6-second clip, generated in the background.',
      KinetoVideoState.generating =>
        'MiniMax is generating the video. It usually takes a few minutes; you can keep working, '
            'and closing Kineto does not lose it.',
      KinetoVideoState.ready => 'The video is ready. Open it, then lock it or generate another.',
      KinetoVideoState.locked => 'The video is locked as production truth.',
      KinetoVideoState.stale =>
        'The video was made from a keyframe that is no longer locked; regenerate it.',
    };
  }
}
