import 'dart:io';

import 'package:flutter/material.dart';
import 'package:wabisabi/wabisabi.dart';

import 'demo_workspace.dart';
import 'native/kineto_engine.dart';

void main() {
  WidgetsFlutterBinding.ensureInitialized();
  final engine = KinetoEngine.open();
  try {
    final project = _openDemoProject();
    runApp(KinetoApp(engine: engine, project: project));
  } on Object {
    engine.close();
    rethrow;
  }
}

KinetoProjectSession _openDemoProject() {
  final target = Directory(_demoProjectPath());
  if (target.existsSync()) {
    return KinetoProjectSession.open(target.path);
  }

  target.parent.createSync(recursive: true);
  try {
    return KinetoProjectSession.createTextProject(
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
    if (error.code == 103 && target.existsSync()) {
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
      return '$localAppData${separator}Kineto${separator}demo-project';
    }
  } else if (Platform.isMacOS) {
    final home = environment['HOME'];
    if (home != null && home.isNotEmpty) {
      return '$home${separator}Library${separator}Application Support${separator}Kineto${separator}demo-project';
    }
  } else {
    final stateHome = environment['XDG_STATE_HOME'];
    if (stateHome != null && stateHome.isNotEmpty) {
      return '$stateHome${separator}kineto${separator}demo-project';
    }
    final home = environment['HOME'];
    if (home != null && home.isNotEmpty) {
      return '$home${separator}.local${separator}state${separator}kineto${separator}demo-project';
    }
  }

  return '${Directory.systemTemp.path}${separator}kineto-demo-project';
}

class KinetoApp extends StatefulWidget {
  const KinetoApp({required this.engine, required this.project, super.key});

  final KinetoEngine engine;
  final KinetoProjectSession project;

  @override
  State<KinetoApp> createState() => _KinetoAppState();
}

class _KinetoAppState extends State<KinetoApp> {
  @override
  void dispose() {
    widget.project.close();
    widget.engine.close();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Kineto',
      theme: WabTheme.materialTheme(lightTheme: false),
      home: DemoWorkspaceScreen(
        engine: widget.engine,
        project: widget.project,
      ),
    );
  }
}
