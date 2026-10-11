import 'dart:async';
import 'dart:io';
import 'dart:ui' show AppExitResponse;

import 'package:flutter/material.dart';
import 'package:wabisabi/wabisabi.dart';

import 'demo_workspace.dart';
import 'handoff_workspace.dart';
import 'kineto_layout.dart';
import 'kineto_work.dart';
import 'media_player.dart';
import 'native/kineto_engine.dart';
import 'preferences_workspace.dart';
import 'production_workspace.dart';
import 'project_busy.dart';
import 'screenplay_workspace.dart';
import 'settings_workspace.dart';
import 'shot_plan_workspace.dart';
import 'voices_workspace.dart';

Future<void> main() => run();

/// Open the demo project and run the app. [pickImage] asks the director for
/// an image to import; the system's open dialog unless a test gives one.
Future<void> run({Future<String?> Function()? pickImage}) async {
  WidgetsFlutterBinding.ensureInitialized();
  // Clips and takes play inside Kineto, with libmpv; without it the rest of
  // Kineto still works, and every play says what is missing.
  final mediaProblem = MediaKitEngine.initialize();
  final media = mediaProblem == null ? const MediaKitEngine() : UnavailableMedia(mediaProblem);
  final engine = KinetoEngine.open();
  // Outside agents the director let in are served again, beside the
  // project; Settings says why when they cannot be.
  unawaited(resumeKinetoServingInBackground().then((_) {}, onError: (Object _) {}));
  try {
    final project = await _openDemoProject();
    runApp(
      KinetoApp(
        engine: engine,
        project: project,
        exportFolder: [Directory(_demoProjectPath()).parent.path, 'exports'].join(Platform.pathSeparator),
        pickImage: pickImage ?? pickImageFile,
        media: media,
      ),
    );
  } on KinetoProjectException catch (error) {
    // A project Kineto cannot hold as its only writer is never opened for
    // writing; explain why instead of opening it or crashing.
    switch (error.code) {
      case KinetoProjectException.busy:
        engine.close();
        runApp(KinetoProjectBusyApp(projectPath: _demoProjectPath()));
      case KinetoProjectException.lockUnavailable:
        engine.close();
        runApp(KinetoProjectLockUnavailableApp(projectPath: _demoProjectPath()));
      default:
        // A provider chosen in settings that cannot be used here, such as
        // an agent that is not installed, must not keep the director out
        // of the settings that change it.
        runApp(
          KinetoApp(
            engine: engine,
            openProblem: error.message,
            exportFolder: [Directory(_demoProjectPath()).parent.path, 'exports'].join(Platform.pathSeparator),
            pickImage: pickImage ?? pickImageFile,
            media: media,
          ),
        );
    }
  } on Object {
    engine.close();
    rethrow;
  }
}

Future<KinetoProjectSession> _openDemoProject() async {
  final target = Directory(_demoProjectPath());
  if (await target.exists()) {
    return KinetoProjectSession.open(target.path);
  }

  await target.parent.create(recursive: true);
  try {
    return await KinetoProjectSession.createTextProject(
      path: target.path,
      projectId: 'kineto_demo',
      title: 'Kineto Demo Project',
      createdAt: DateTime.now().toUtc().toIso8601String(),
      language: 'en',
      sourceText:
          'A filmmaker waits across the table. The final line lands, and the reaction matters more than the dialogue.\n',
    );
  } on KinetoProjectException catch (error) {
    // Another process may have created the same canonical demo project between
    // the existence check and create. Never overwrite it; reopen the winner.
    if (error.code == KinetoProjectException.alreadyExists &&
        await target.exists()) {
      return KinetoProjectSession.open(target.path);
    }
    rethrow;
  }
}

String _demoProjectPath() {
  final environment = Platform.environment;
  final separator = Platform.pathSeparator;

  if (Platform.isWindows) {
    final localAppData = environment['LOCALAPPDATA'];
    if (localAppData != null && localAppData.isNotEmpty) {
      return [localAppData, 'Kineto', 'demo-project'].join(separator);
    }
  } else if (Platform.isMacOS) {
    final home = environment['HOME'];
    if (home != null && home.isNotEmpty) {
      return [
        home,
        'Library',
        'Application Support',
        'Kineto',
        'demo-project',
      ].join(separator);
    }
  } else {
    final stateHome = environment['XDG_STATE_HOME'];
    if (stateHome != null && stateHome.isNotEmpty) {
      return [stateHome, 'kineto', 'demo-project'].join(separator);
    }
    final home = environment['HOME'];
    if (home != null && home.isNotEmpty) {
      return [home, '.local', 'state', 'kineto', 'demo-project'].join(separator);
    }
  }

  return [Directory.systemTemp.path, 'kineto-demo-project'].join(separator);
}

class KinetoApp extends StatefulWidget {
  const KinetoApp({
    required this.engine,
    this.project,
    this.openProblem,
    this.openProject = _openDemoProject,
    this.exportFolder = '',
    this.pickImage = pickImageFile,
    this.media = const MediaKitEngine(),
    super.key,
  });

  final KinetoEngine engine;

  /// What plays sounds and clips inside Kineto.
  final KinetoMediaEngine media;

  /// The open project; `null` when it could not be opened, and then
  /// [openProblem] says why.
  final KinetoProjectSession? project;
  final String? openProblem;

  /// Opens the project again, which chooses its providers again.
  final Future<KinetoProjectSession> Function() openProject;

  /// Asks the director for an image to import.
  final Future<String?> Function() pickImage;

  /// Where production packages are made unless the director names another
  /// folder.
  final String exportFolder;

  @override
  State<KinetoApp> createState() => _KinetoAppState();
}

/// The workspaces the director moves between.
enum KinetoWorkspace { screenplay, production, voices, shotPlan, shots, handoff, preferences, settings }

class _KinetoAppState extends State<KinetoApp> {
  bool _darkMode = true;
  KinetoWorkspace _workspace = KinetoWorkspace.screenplay;
  late final AppLifecycleListener _lifecycle;
  KinetoProjectSession? _project;
  String? _openProblem;
  bool _reopening = false;

  /// A provider choice, a key or the configuration changed in settings
  /// since the project was opened; its providers are chosen when it opens.
  bool _choicesChanged = false;
  Timer? _waitForIdle;

  /// The open project's requests, shown on their buttons and in the status
  /// bar whatever workspace is open.
  KinetoWork? _work;

  /// The sound playing, whatever workspace is open.
  late final KinetoSound _sound = KinetoSound(widget.media);

  @override
  void initState() {
    super.initState();
    _project = widget.project;
    _openProblem = widget.openProblem;
    if (_project != null) _work = KinetoWork();
    if (_project == null) _workspace = KinetoWorkspace.settings;
    // Close the session before the process exits so its worker stops
    // reporting and the project's writer lock is released cleanly.
    _lifecycle = AppLifecycleListener(onExitRequested: _closeNative);
  }

  Future<AppExitResponse> _closeNative() async {
    _waitForIdle?.cancel();
    _project?.close();
    widget.engine.close();
    return AppExitResponse.exit;
  }

  void _show(KinetoWorkspace workspace) {
    final leavingSettings = _workspace == KinetoWorkspace.settings && workspace != KinetoWorkspace.settings;
    setState(() => _workspace = workspace);
    if (leavingSettings && (_choicesChanged || (_project == null && !_reopening))) _reopen();
  }

  /// Open the project again so it uses what was chosen in settings. Work
  /// under way finishes first: a choice never cuts a request off. Videos
  /// already submitted are watched again by the new session.
  Future<void> _reopen() async {
    _waitForIdle?.cancel();
    if (_reopening) return;
    final current = _project;
    if (current != null && (current.working || (_work?.busy ?? false))) {
      _waitForIdle = Timer(const Duration(seconds: 1), _reopen);
      return;
    }
    final finished = _work;
    setState(() {
      _reopening = true;
      _choicesChanged = false;
      _project = null;
      _work = null;
      _openProblem = null;
    });
    // The pages and the status bar let go of it as this frame rebuilds them.
    WidgetsBinding.instance.addPostFrameCallback((_) => finished?.dispose());
    current?.close();
    KinetoProjectSession? opened;
    String? problem;
    try {
      opened = await widget.openProject();
    } on KinetoProjectException catch (error) {
      problem = error.message;
    } on Object catch (error) {
      problem = error.toString();
    }
    if (!mounted) {
      opened?.close();
      return;
    }
    setState(() {
      _reopening = false;
      _project = opened;
      if (opened != null) _work = KinetoWork();
      _openProblem = problem;
    });
  }

  void _toggleTheme() {
    setState(() {
      _darkMode = !_darkMode;
    });
  }

  @override
  void dispose() {
    _lifecycle.dispose();
    _waitForIdle?.cancel();
    _project?.close();
    _work?.dispose();
    _sound.dispose();
    widget.engine.close();
    super.dispose();
  }

  static const Map<KinetoWorkspace, String> _labels = <KinetoWorkspace, String>{
    KinetoWorkspace.screenplay: 'Story & screenplay',
    KinetoWorkspace.production: 'Bible, style & cast',
    KinetoWorkspace.voices: 'Voices & dialogue',
    KinetoWorkspace.shotPlan: 'Shot plan',
    KinetoWorkspace.shots: 'Shots',
    KinetoWorkspace.handoff: 'Handoff',
    KinetoWorkspace.preferences: 'Preferences',
    KinetoWorkspace.settings: 'Settings',
  };

  static const Map<KinetoWorkspace, IconData> _icons = <KinetoWorkspace, IconData>{
    KinetoWorkspace.screenplay: Icons.article_outlined,
    KinetoWorkspace.production: Icons.face_retouching_natural_outlined,
    KinetoWorkspace.voices: Icons.record_voice_over_outlined,
    KinetoWorkspace.shotPlan: Icons.view_timeline_outlined,
    KinetoWorkspace.shots: Icons.movie_outlined,
    KinetoWorkspace.handoff: Icons.inventory_2_outlined,
    KinetoWorkspace.preferences: Icons.tune_outlined,
    KinetoWorkspace.settings: Icons.settings_outlined,
  };

  /// The workspaces down the left side of the window.
  Widget _sidebar() {
    String? title;
    try {
      title = _project?.title;
    } on Object {
      title = null;
    }
    return KinetoSidebar(
      places: <KinetoPlace>[
        for (final MapEntry(key: workspace, value: label) in _labels.entries)
          KinetoPlace(page: workspace.name, label: label, icon: _icons[workspace]!),
      ],
      current: _workspace.name,
      onShow: (page) => _show(KinetoWorkspace.values.byName(page)),
      projectTitle: title,
      isDarkMode: _darkMode,
      onThemeToggle: _toggleTheme,
    );
  }

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Kineto',
      theme: WabTheme.materialTheme(lightTheme: !_darkMode),
      // Any text can be selected and copied, an error included. The status
      // bar is outside every workspace, so work under way is in sight
      // whichever is open.
      home: SelectionArea(
        child: Column(
          children: <Widget>[
            Expanded(
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: <Widget>[
                  _sidebar(),
                  Expanded(child: _home()),
                ],
              ),
            ),
            KinetoStatusBar(
              work: _work,
              pageLabel: (page) => _labels[KinetoWorkspace.values.byName(page)] ?? page,
              onShow: (page) => _show(KinetoWorkspace.values.byName(page)),
            ),
          ],
        ),
      ),
      // Above every route, so a button in a dialog shows its work too.
      builder: (context, navigator) => KinetoMedia(
        sound: _sound,
        child: KinetoWorkScope(work: _work, child: navigator ?? const SizedBox.shrink()),
      ),
    );
  }

  Widget _home() {
    if (_workspace == KinetoWorkspace.settings) {
      return SettingsWorkspaceScreen(
        onChanged: () => _choicesChanged = true,
      );
    }
    final project = _project;
    final work = _work;
    if (project == null || work == null) {
      return ProjectUnavailableScreen(
        opening: _reopening,
        problem: _openProblem,
      );
    }
    // A project opened again is a new session: every workspace starts over
    // with it rather than keep the closed one.
    return KeyedSubtree(
      key: ObjectKey(project),
      child: switch (_workspace) {
        KinetoWorkspace.screenplay => ScreenplayWorkspaceScreen(
            project: project,
            work: work,
          ),
        KinetoWorkspace.production => ProductionWorkspaceScreen(
            project: project,
            work: work,
            pickImage: widget.pickImage,
          ),
        KinetoWorkspace.voices => VoicesWorkspaceScreen(
            project: project,
            work: work,
          ),
        KinetoWorkspace.preferences => PreferencesWorkspaceScreen(
            project: project,
            work: work,
          ),
        KinetoWorkspace.shotPlan => ShotPlanWorkspaceScreen(
            project: project,
            work: work,
          ),
        KinetoWorkspace.shots => DemoWorkspaceScreen(
            engine: widget.engine,
            project: project,
            work: work,
          ),
        KinetoWorkspace.settings => throw StateError('settings are built above'),
        KinetoWorkspace.handoff => HandoffWorkspaceScreen(
            project: project,
            work: work,
            exportFolder: widget.exportFolder,
          ),
      },
    );
  }
}
