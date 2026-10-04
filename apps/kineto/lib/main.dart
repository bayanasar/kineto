import 'dart:io';
import 'dart:ui' show AppExitResponse;

import 'package:flutter/material.dart';
import 'package:wabisabi/wabisabi.dart';

import 'demo_workspace.dart';
import 'native/kineto_engine.dart';
import 'production_workspace.dart';
import 'project_busy.dart';
import 'screenplay_workspace.dart';
import 'shot_plan_workspace.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  final engine = KinetoEngine.open();
  try {
    final project = await _openDemoProject();
    runApp(KinetoApp(engine: engine, project: project));
  } on KinetoProjectException catch (error) {
    engine.close();
    // A project Kineto cannot hold as its only writer is never opened for
    // writing; explain why instead of opening it or crashing.
    switch (error.code) {
      case KinetoProjectException.busy:
        runApp(KinetoProjectBusyApp(projectPath: _demoProjectPath()));
      case KinetoProjectException.lockUnavailable:
        runApp(KinetoProjectLockUnavailableApp(projectPath: _demoProjectPath()));
      default:
        rethrow;
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
  const KinetoApp({required this.engine, required this.project, super.key});

  final KinetoEngine engine;
  final KinetoProjectSession project;

  @override
  State<KinetoApp> createState() => _KinetoAppState();
}

/// The workspaces the director moves between.
enum KinetoWorkspace { screenplay, production, shotPlan, shots }

class _KinetoAppState extends State<KinetoApp> {
  bool _darkMode = true;
  KinetoWorkspace _workspace = KinetoWorkspace.screenplay;
  late final AppLifecycleListener _lifecycle;

  @override
  void initState() {
    super.initState();
    // Close the session before the process exits so its worker stops
    // reporting and the project's writer lock is released cleanly.
    _lifecycle = AppLifecycleListener(onExitRequested: _closeNative);
  }

  Future<AppExitResponse> _closeNative() async {
    widget.project.close();
    widget.engine.close();
    return AppExitResponse.exit;
  }

  void _toggleTheme() {
    setState(() {
      _darkMode = !_darkMode;
    });
  }

  @override
  void dispose() {
    _lifecycle.dispose();
    widget.project.close();
    widget.engine.close();
    super.dispose();
  }

  Widget _navigation() => Wrap(
        spacing: 12,
        children: <Widget>[
          for (final (workspace, label) in <(KinetoWorkspace, String)>[
            (KinetoWorkspace.screenplay, 'Story & screenplay'),
            (KinetoWorkspace.production, 'Bible, style & cast'),
            (KinetoWorkspace.shotPlan, 'Shot plan'),
            (KinetoWorkspace.shots, 'Shots'),
          ])
            WabButton(
              key: ValueKey<String>('workspace-${workspace.name}'),
              kind: workspace == _workspace ? WabMaterialKind.seal : WabMaterialKind.zhuwen,
              onPressed: () => setState(() => _workspace = workspace),
              child: Text(label),
            ),
        ],
      );

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Kineto',
      theme: WabTheme.materialTheme(lightTheme: !_darkMode),
      home: switch (_workspace) {
        KinetoWorkspace.screenplay => ScreenplayWorkspaceScreen(
            project: widget.project,
            navigation: _navigation(),
            isDarkMode: _darkMode,
            onThemeToggle: _toggleTheme,
          ),
        KinetoWorkspace.production => ProductionWorkspaceScreen(
            project: widget.project,
            navigation: _navigation(),
            isDarkMode: _darkMode,
            onThemeToggle: _toggleTheme,
          ),
        KinetoWorkspace.shotPlan => ShotPlanWorkspaceScreen(
            project: widget.project,
            navigation: _navigation(),
            isDarkMode: _darkMode,
            onThemeToggle: _toggleTheme,
          ),
        KinetoWorkspace.shots => DemoWorkspaceScreen(
            engine: widget.engine,
            project: widget.project,
            navigation: _navigation(),
            isDarkMode: _darkMode,
            onThemeToggle: _toggleTheme,
          ),
      },
    );
  }
}
