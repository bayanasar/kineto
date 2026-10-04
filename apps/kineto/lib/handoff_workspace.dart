import 'dart:io';

import 'package:flutter/material.dart';
import 'package:wabisabi/wabisabi.dart';

import 'native/kineto_engine.dart';

/// [bytes] for a person, for example `12.4 MB`.
String formatBytes(int bytes) {
  const units = <String>['bytes', 'KB', 'MB', 'GB', 'TB'];
  var value = bytes.toDouble();
  var unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit++;
  }
  return unit == 0 ? '$bytes bytes' : '${value.toStringAsFixed(1)} ${units[unit]}';
}

/// Open [path], a folder or file, with the platform's default application.
Future<void> openInSystem(String path) async {
  final (executable, arguments) = Platform.isMacOS
      ? ('open', <String>[path])
      : Platform.isWindows
          ? ('explorer', <String>[path])
          : ('xdg-open', <String>[path]);
  await Process.start(executable, arguments, mode: ProcessStartMode.detached);
}

class HandoffWorkspaceScreen extends StatefulWidget {
  const HandoffWorkspaceScreen({
    required this.project,
    required this.exportFolder,
    this.navigation,
    this.isDarkMode = true,
    this.onThemeToggle,
    super.key,
  });

  final KinetoProjectSession project;

  /// Where packages are made unless the director names another folder.
  final String exportFolder;
  final Widget? navigation;
  final bool isDarkMode;
  final VoidCallback? onThemeToggle;

  @override
  State<HandoffWorkspaceScreen> createState() => _HandoffWorkspaceScreenState();
}

class _HandoffWorkspaceScreenState extends State<HandoffWorkspaceScreen> {
  late List<KinetoSceneProgress> _scenes;
  late KinetoStorage _storage;
  late KinetoExportState _export;
  bool _working = false;
  String _workingLabel = 'Working';
  String? _error;

  @override
  void initState() {
    super.initState();
    _read();
  }

  void _read() {
    _scenes = widget.project.sceneProgress;
    _storage = widget.project.storage;
    _export = widget.project.exportState;
  }

  Future<void> _run(Future<void> Function() action, String label) async {
    if (_working) return;
    setState(() {
      _working = true;
      _workingLabel = label;
      _error = null;
    });
    String? error;
    try {
      await action();
    } on KinetoProjectException catch (failure) {
      error = failure.message;
    } on Object catch (failure) {
      error = failure.toString();
    }
    if (!mounted) return;
    setState(() {
      _working = false;
      _read();
      _error = error;
    });
  }

  @override
  Widget build(BuildContext context) => HandoffWorkspaceView(
        scenes: _scenes,
        storage: _storage,
        export: _export,
        exportFolder: widget.exportFolder,
        working: _working,
        workingLabel: _workingLabel,
        error: _error,
        navigation: widget.navigation,
        isDarkMode: widget.isDarkMode,
        onThemeToggle: widget.onThemeToggle,
        onPlanCollection: () => _run(widget.project.planCollection, 'Working out what can be let go'),
        onCollect: () => _run(widget.project.collectMedia, 'Letting go'),
        onVerify: () => _run(widget.project.verifyMedia, 'Reading every file'),
        onExport: (folder, name, {required partial, required replace, required timeline}) => _run(
          () async {
            Directory(folder).createSync(recursive: true);
            await widget.project.exportPackage(
              destination: folder,
              name: name,
              partial: partial,
              replace: replace,
              timeline: timeline,
            );
          },
          'Exporting',
        ),
        onOpenFolder: openInSystem,
      );
}

class HandoffWorkspaceView extends StatefulWidget {
  const HandoffWorkspaceView({
    required this.scenes,
    required this.storage,
    required this.export,
    required this.exportFolder,
    required this.onPlanCollection,
    required this.onCollect,
    required this.onVerify,
    required this.onExport,
    this.onOpenFolder,
    this.working = false,
    this.workingLabel = 'Working',
    this.error,
    this.navigation,
    this.isDarkMode = true,
    this.onThemeToggle,
    super.key,
  });

  final List<KinetoSceneProgress> scenes;
  final KinetoStorage storage;
  final KinetoExportState export;
  final String exportFolder;
  final bool working;
  final String workingLabel;
  final String? error;
  final Widget? navigation;
  final bool isDarkMode;
  final VoidCallback? onThemeToggle;
  final VoidCallback onPlanCollection;
  final VoidCallback onCollect;
  final VoidCallback onVerify;
  final void Function(
    String folder,
    String name, {
    required bool partial,
    required bool replace,
    required bool timeline,
  }) onExport;
  final ValueChanged<String>? onOpenFolder;

  @override
  State<HandoffWorkspaceView> createState() => _HandoffWorkspaceViewState();
}

class _HandoffWorkspaceViewState extends State<HandoffWorkspaceView> {
  late final TextEditingController _folder = TextEditingController(text: widget.exportFolder);
  final TextEditingController _name = TextEditingController(text: 'dailies');
  bool _partial = false;
  bool _replace = false;
  bool _timeline = true;

  @override
  void dispose() {
    _folder.dispose();
    _name.dispose();
    super.dispose();
  }

  bool get _idle => !widget.working;

  @override
  Widget build(BuildContext context) => WabScaffold(
        appBar: WabAppBar(
          title: const Text('Kineto'),
          action: widget.onThemeToggle == null
              ? null
              : WabToggleButton(
                  text: const Text('Dark mode'),
                  isOn: widget.isDarkMode,
                  pair: WabTogglePair.sealZhuwen,
                  callback: widget.onThemeToggle,
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
                  if (widget.navigation != null) ...<Widget>[widget.navigation!, const SizedBox(height: 20)],
                  if (widget.working) ...<Widget>[
                    Align(
                      alignment: Alignment.centerLeft,
                      child: WabStatusBadge(widget.workingLabel, kind: WabBadgeKind.progress),
                    ),
                    const SizedBox(height: 12),
                  ],
                  if (widget.error != null) ...<Widget>[
                    WabPanel(title: 'Not done', child: Text(widget.error!)),
                    const SizedBox(height: 20),
                  ],
                  _scenesPanel(context),
                  const SizedBox(height: 20),
                  _exportPanel(context),
                  const SizedBox(height: 20),
                  _storagePanel(context),
                ],
              ),
            ),
          ),
        ),
      );

  // ---------------------------------------------------------------- scenes

  Widget _scenesPanel(BuildContext context) => WabPanel(
        title: 'Scenes',
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: <Widget>[
            if (widget.scenes.isEmpty) const Text('No shots are in play yet; lock a shot plan first.'),
            for (final scene in widget.scenes) ...<Widget>[
              Row(
                children: <Widget>[
                  Expanded(
                    child: Text(
                      scene.sceneId.replaceAll('_', ' '),
                      style: Theme.of(context).textTheme.titleSmall,
                    ),
                  ),
                  WabStatusBadge(
                    scene.complete ? 'Complete' : 'In progress',
                    key: ValueKey<String>('scene-${scene.sceneId}'),
                    kind: scene.complete ? WabBadgeKind.done : WabBadgeKind.progress,
                  ),
                ],
              ),
              const SizedBox(height: 8),
              for (final shot in scene.shots)
                Padding(
                  padding: const EdgeInsets.only(bottom: 6),
                  child: Wrap(
                    spacing: 12,
                    runSpacing: 6,
                    crossAxisAlignment: WrapCrossAlignment.center,
                    children: <Widget>[
                      SizedBox(width: 220, child: Text(shot.shotId)),
                      _stage('Keyframe', shot.keyframe),
                      _stage('Video', shot.video),
                      if (_missing(shot) case final missing?) Text(missing),
                    ],
                  ),
                ),
              const SizedBox(height: 12),
            ],
          ],
        ),
      );

  static Widget _stage(String what, KinetoStage stage) => WabStatusBadge(
        '$what · ${switch (stage) {
          KinetoStage.missing => 'none',
          KinetoStage.made => 'to choose',
          KinetoStage.locked => 'locked',
          KinetoStage.stale => 'stale',
        }}',
        kind: switch (stage) {
          KinetoStage.missing => WabBadgeKind.neutral,
          KinetoStage.made => WabBadgeKind.progress,
          KinetoStage.locked => WabBadgeKind.done,
          KinetoStage.stale => WabBadgeKind.warning,
        },
      );

  /// What the shot still needs before its scene is complete.
  static String? _missing(KinetoShotProgress shot) => switch ((shot.keyframe, shot.video)) {
        (_, KinetoStage.locked) => null,
        (KinetoStage.missing || KinetoStage.made, _) => 'Needs a locked keyframe',
        (KinetoStage.stale, _) => 'Its keyframe no longer follows the shot',
        (_, KinetoStage.missing) => 'Needs a video',
        (_, KinetoStage.made) => 'Needs a take chosen and locked',
        (_, KinetoStage.stale) => 'Its video no longer follows the shot',
      };

  // ---------------------------------------------------------------- export

  Widget _exportPanel(BuildContext context) {
    final export = widget.export;
    final total = export.ready + export.missing.length;
    final last = export.last;
    return WabPanel(
      title: 'Production package',
      trailing: WabStatusBadge(
        export.complete ? 'Ready' : '${export.ready} of $total shots ready',
        kind: export.complete ? WabBadgeKind.done : WabBadgeKind.neutral,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          const Text(
            'Each locked video with a sidecar of what made it, and a manifest with scenes, shot order '
            'and runtime: everything an editor needs without Kineto. With the timeline, the shots and '
            'their dubbed lines are laid end to end as timeline.otio (OpenTimelineIO) and timeline.edl '
            '(CMX 3600), ready to open in an editing application.',
          ),
          if (export.missing.isNotEmpty) ...<Widget>[
            const SizedBox(height: 8),
            for (final missing in export.missing) Text('${missing.shotId}: ${missing.reason}.'),
          ],
          const SizedBox(height: 12),
          TextField(
            key: const ValueKey<String>('export-folder'),
            controller: _folder,
            decoration: wabInputDecoration(hintText: 'Folder the package is made in'),
          ),
          const SizedBox(height: 8),
          TextField(
            key: const ValueKey<String>('export-name'),
            controller: _name,
            decoration: wabInputDecoration(hintText: 'Package name'),
          ),
          const SizedBox(height: 8),
          Wrap(
            spacing: 24,
            runSpacing: 8,
            children: <Widget>[
              if (export.missing.isNotEmpty)
                WabCheckbox(
                  key: const ValueKey<String>('export-partial'),
                  value: _partial,
                  onChanged: (value) => setState(() => _partial = value ?? false),
                  label: 'Export only the ready shots',
                ),
              WabCheckbox(
                key: const ValueKey<String>('export-timeline'),
                value: _timeline,
                onChanged: (value) => setState(() => _timeline = value ?? false),
                label: 'Include the timeline (OTIO and EDL)',
              ),
              WabCheckbox(
                key: const ValueKey<String>('export-replace'),
                value: _replace,
                onChanged: (value) => setState(() => _replace = value ?? false),
                label: 'Replace a package of that name',
              ),
            ],
          ),
          const SizedBox(height: 8),
          Wrap(
            spacing: 12,
            runSpacing: 12,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: <Widget>[
              WabButton(
                key: const ValueKey<String>('export'),
                kind: WabMaterialKind.seal,
                onPressed: _idle && export.ready > 0 && (export.missing.isEmpty || _partial)
                    ? () => widget.onExport(
                          _folder.text.trim(),
                          _name.text.trim(),
                          partial: export.missing.isNotEmpty && _partial,
                          replace: _replace,
                          timeline: _timeline,
                        )
                    : null,
                child: Text(export.ready == 1 ? 'Export 1 shot' : 'Export ${export.ready} shots'),
              ),
              if (last != null) ...<Widget>[
                Text(
                  'Wrote ${last.path} · ${last.shots} shot${last.shots == 1 ? '' : 's'}'
                  '${last.lines == 0 ? '' : ' · ${last.lines} dubbed line${last.lines == 1 ? '' : 's'}'}'
                  ' · ${formatBytes(last.bytes)}${last.complete ? '' : ' · partial'}',
                ),
                if (widget.onOpenFolder != null)
                  WabButton(
                    key: const ValueKey<String>('open-package'),
                    kind: WabMaterialKind.paper,
                    onPressed: () => widget.onOpenFolder!(last.path),
                    child: const Text('Open folder'),
                  ),
              ],
            ],
          ),
        ],
      ),
    );
  }

  // --------------------------------------------------------------- storage

  Widget _storagePanel(BuildContext context) {
    final storage = widget.storage;
    final plan = storage.plan;
    return WabPanel(
      title: 'Storage',
      trailing: Text(formatBytes(storage.usageBytes)),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          Text(
            'Media takes ${formatBytes(storage.usageBytes)}, each file counted once. Locked work and what '
            'it was made from are always kept, and so are the latest ${storage.recentTakes} takes of each '
            'shot, character and style.',
          ),
          if (storage.problem != null) ...<Widget>[const SizedBox(height: 8), Text(storage.problem!)],
          if (storage.owners.isNotEmpty) ...<Widget>[
            const SizedBox(height: 8),
            for (final (owner, bytes) in storage.owners) Text('$owner · ${formatBytes(bytes)}'),
          ],
          if (storage.missing.isNotEmpty) ...<Widget>[
            const SizedBox(height: 12),
            Text('Missing files', style: Theme.of(context).textTheme.titleSmall),
            for (final item in storage.missing) Text('${item.artifactId} (${item.owner}) is gone.'),
          ],
          if (storage.mismatched.isNotEmpty) ...<Widget>[
            const SizedBox(height: 12),
            Text('Changed files', style: Theme.of(context).textTheme.titleSmall),
            for (final item in storage.mismatched)
              Text('${item.artifactId} (${item.owner}) is not what its record says.'),
          ],
          const SizedBox(height: 12),
          Wrap(
            spacing: 12,
            runSpacing: 12,
            children: <Widget>[
              WabButton(
                key: const ValueKey<String>('plan-collection'),
                kind: WabMaterialKind.paper,
                onPressed: _idle ? widget.onPlanCollection : null,
                child: const Text('Show what can be let go'),
              ),
              WabButton(
                key: const ValueKey<String>('verify-media'),
                kind: WabMaterialKind.paper,
                onPressed: _idle ? widget.onVerify : null,
                child: const Text('Check every file'),
              ),
            ],
          ),
          if (plan != null) ...<Widget>[
            const SizedBox(height: 12),
            if (plan.items.isEmpty)
              const Text('Nothing can be let go: every file is locked, needed, recent or undecided.')
            else ...<Widget>[
              Text(
                '${plan.items.length} file${plan.items.length == 1 ? '' : 's'} can be let go, freeing '
                '${formatBytes(plan.bytes)}. Their records, prompts and provenance stay.',
              ),
              const SizedBox(height: 8),
              for (final item in plan.items) Text('${item.artifactId} (${item.owner}): ${item.reason}.'),
              const SizedBox(height: 8),
              WabButton(
                key: const ValueKey<String>('collect'),
                kind: WabMaterialKind.zhuwen,
                onPressed: _idle ? () => _confirmCollect(context, plan) : null,
                child: Text('Let go of ${plan.items.length} file${plan.items.length == 1 ? '' : 's'}…'),
              ),
            ],
          ],
        ],
      ),
    );
  }

  Future<void> _confirmCollect(BuildContext context, KinetoCollection plan) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text('Let go of ${plan.items.length} file${plan.items.length == 1 ? '' : 's'}?'),
        content: Text(
          'This frees ${formatBytes(plan.bytes)}. The files are deleted; their records stay, and '
          'nothing locked or needed by locked work is touched.',
        ),
        actions: <Widget>[
          TextButton(onPressed: () => Navigator.pop(context), child: const Text('Cancel')),
          TextButton(
            key: const ValueKey<String>('collect-confirm'),
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Let go'),
          ),
        ],
      ),
    );
    if (confirmed ?? false) widget.onCollect();
  }
}
