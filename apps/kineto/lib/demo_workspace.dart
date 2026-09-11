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

class DemoWorkspaceScreen extends StatefulWidget {
  const DemoWorkspaceScreen({
    required this.engine,
    required this.project,
    this.isDarkMode = true,
    this.onThemeToggle,
    super.key,
  });

  final KinetoEngine engine;
  final KinetoProjectSession project;
  final bool isDarkMode;
  final VoidCallback? onThemeToggle;

  @override
  State<DemoWorkspaceScreen> createState() => _DemoWorkspaceScreenState();
}

class _DemoWorkspaceScreenState extends State<DemoWorkspaceScreen> {
  late final KinetoEngineSnapshot _engineSnapshot;
  late List<KinetoShotSnapshot> _shotSnapshots;
  int _activeShotIndex = 0;
  String? _error;

  @override
  void initState() {
    super.initState();
    _engineSnapshot = widget.engine.snapshot;
    _shotSnapshots = _readShotSnapshots();
  }

  List<KinetoShotSnapshot> _readShotSnapshots() =>
      List<KinetoShotSnapshot>.generate(
        KinetoProjectSession.shotCount,
        widget.project.shotSnapshot,
        growable: false,
      );

  void _switchShot(int index) {
    setState(() {
      _activeShotIndex = index;
      _error = null;
    });
  }

  void _run(void Function() action) {
    try {
      action();
      setState(() {
        _shotSnapshots = _readShotSnapshots();
        _error = null;
      });
    } on Object catch (error) {
      setState(() {
        _shotSnapshots = _readShotSnapshots();
        _error = error.toString();
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final snapshot = _shotSnapshots[_activeShotIndex];
    return DemoWorkspaceView(
      engineSnapshot: _engineSnapshot,
      shotSnapshot: snapshot,
      shotSnapshots: _shotSnapshots,
      activeShotIndex: _activeShotIndex,
      isDarkMode: widget.isDarkMode,
      error: _error,
      onThemeToggle: widget.onThemeToggle,
      onShotChanged: _switchShot,
      onDirectionChanged: (direction) => _run(
        () => widget.project.setShotDirection(_activeShotIndex, direction),
      ),
      onGenerate: () => _run(
        () => widget.project.generateShotCandidates(_activeShotIndex),
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
    this.isDarkMode = true,
    this.onThemeToggle,
    this.error,
    super.key,
  });

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
  final String? error;

  @override
  Widget build(BuildContext context) {
    final selectedIndex = shotSnapshot.selectedIndex;
    final canLock = shotSnapshot.generated &&
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
                WabPanel(
                  title: 'Canonical demo project',
                  trailing: Text('Native ABI ${engineSnapshot.abiVersion}'),
                  child: const Text(
                    'A deterministic local vertical slice. Work through two independent shots, choose direction, '
                    'generate candidates, select and lock, then reopen Kineto to verify canonical project state.',
                  ),
                ),
                const SizedBox(height: 20),
                WabPanel(
                  title: 'Scene 001',
                  trailing: WabStatusBadge(
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
                              onPressed: shotSnapshot.locked
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
                      if (!shotSnapshot.generated)
                        WabElevatedButton(
                          text: const Text('Generate 3 deterministic candidates'),
                          callback: onGenerate,
                        )
                      else
                        Row(
                          children: <Widget>[
                            Expanded(child: Text(_generationStatus(shotSnapshot))),
                            const SizedBox(width: 16),
                            WabButton(
                              kind: WabMaterialKind.zhuwen,
                              onPressed: shotSnapshot.locked ? null : onGenerate,
                              child: const Text('Regenerate'),
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
                          final content = shot.candidates[index];
                          final selected = selectedIndex == index;
                          return SizedBox(
                            width: 300,
                            child: WabCollectionCard(
                              icon: Icon(content.icon, size: 34),
                              title: content.title,
                              description: content.description,
                              buttonLabel: shotSnapshot.locked
                                  ? (selected ? 'Locked' : 'Candidate')
                                  : (selected ? 'Selected' : 'Select'),
                              highlighted: selected,
                              onPressed: shotSnapshot.locked
                                  ? null
                                  : () => onSelect(index),
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
                          onPressed: onReset,
                          expand: true,
                          child: const Text('Reset shot'),
                        ),
                      ),
                    ],
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
