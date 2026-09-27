import 'dart:convert';
import 'dart:io';

Future<void> main() async {
  final appRoot = Directory.current.absolute;
  final pubspec = File(_join(appRoot.path, 'pubspec.yaml'));
  if (!pubspec.existsSync()) {
    throw StateError(
      'Run this probe from apps/kineto so pubspec.yaml is the current package.',
    );
  }

  final sandbox = Directory.systemTemp.createTempSync(
    'kineto-finalizer-ownership-',
  );
  try {
    final fixture = Directory(_join(sandbox.path, 'runner'))..createSync();
    final binDirectory = Directory(_join(fixture.path, 'bin'))
      ..createSync(recursive: true);
    final sourceProbe = File(
      _join(appRoot.path, 'tool', 'native_finalizer_probe.dart'),
    );
    sourceProbe.copySync(
      _join(binDirectory.path, 'native_finalizer_probe.dart'),
    );

    final packagePath = appRoot.path.replaceAll(r'\', '/');
    File(_join(fixture.path, 'pubspec.yaml'))
        .writeAsStringSync(_fixturePubspec(jsonEncode(packagePath)));

    final flutter = _flutterExecutable();
    await _runChecked(flutter, const [
      'pub',
      'get',
      '--offline',
    ], workingDirectory: fixture.path);
    final dart = _dartExecutable();

    for (final mode in const ['finalizer', 'explicit-close']) {
      final marker = File(_join(sandbox.path, '$mode.marker'));
      if (marker.existsSync()) marker.deleteSync();

      await _runChecked(
        dart,
        const ['run', 'bin/native_finalizer_probe.dart'],
        workingDirectory: fixture.path,
        environment: <String, String>{
          ...Platform.environment,
          'KINETO_OWNERSHIP_PROBE_MODE': mode,
          'KINETO_OWNERSHIP_PROBE_FILE': marker.path,
        },
      );

      if (!marker.existsSync()) {
        throw StateError('$mode did not destroy the native engine');
      }
      final events = marker
          .readAsLinesSync()
          .where((line) => line.trim().isNotEmpty)
          .toList(growable: false);
      if (events.length != 1 || events.single != 'destroy') {
        throw StateError(
          '$mode must destroy exactly once; observed events: $events',
        );
      }
    }

    stdout.writeln(
      'KinetoEngine ownership probe passed: finalizer and explicit close '
      'each destroyed exactly once.',
    );
  } finally {
    if (sandbox.existsSync()) {
      sandbox.deleteSync(recursive: true);
    }
  }
}

Future<void> _runChecked(
  String executable,
  List<String> arguments, {
  required String workingDirectory,
  Map<String, String>? environment,
}) async {
  final result = await Process.run(
    executable,
    arguments,
    workingDirectory: workingDirectory,
    environment: environment,
    runInShell: Platform.isWindows,
  );
  if (result.exitCode == 0) return;

  stderr
    ..writeln('Command failed: $executable ${arguments.join(' ')}')
    ..write(result.stdout)
    ..write(result.stderr);
  throw StateError(
    'Command failed with exit code ${result.exitCode}: '
    '$executable ${arguments.join(' ')}',
  );
}

String _flutterExecutable() {
  final flutterRoot = Platform.environment['FLUTTER_ROOT'];
  final binary = Platform.isWindows ? 'flutter.bat' : 'flutter';
  if (flutterRoot == null || flutterRoot.isEmpty) return binary;
  return _join(flutterRoot, 'bin', binary);
}

String _dartExecutable() {
  final flutterRoot = Platform.environment['FLUTTER_ROOT'];
  final binary = Platform.isWindows ? 'dart.exe' : 'dart';
  if (flutterRoot == null || flutterRoot.isEmpty) return binary;
  return _join(flutterRoot, 'bin', 'cache', 'dart-sdk', 'bin', binary);
}

String _fixturePubspec(String quotedPackagePath) =>
    '''
name: kineto_ownership_probe_runner
publish_to: none
environment:
  sdk: '>=3.13.0 <4.0.0'
  flutter: '>=3.47.0'
dependencies:
  flutter:
    sdk: flutter
  kineto:
    path: $quotedPackagePath
hooks:
  user_defines:
    kineto:
      ownership_test_hooks: true
''';

String _join(
  String first,
  String second, [
  String? third,
  String? fourth,
  String? fifth,
  String? sixth,
]) {
  final parts = <String>[first, second, ?third, ?fourth, ?fifth, ?sixth];
  return parts.join(Platform.pathSeparator);
}
