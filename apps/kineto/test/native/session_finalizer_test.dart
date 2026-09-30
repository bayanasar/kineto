import 'dart:convert';
import 'dart:developer';
import 'dart:io';
import 'dart:isolate';

import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/native/kineto_engine.dart';

/// A session reclaimed only by garbage collection must neither strand an
/// answer it still owes nor leave its port keeping the isolate alive.
///
/// Forcing a collection needs the VM service: run with
/// `flutter test --enable-vmservice`. CI sets KINETO_REQUIRE_VM_SERVICE so the
/// test fails there instead of being skipped.
Future<void> main() async {
  final serviceUri = (await Service.getInfo()).serverWebSocketUri;
  final required = Platform.environment['KINETO_REQUIRE_VM_SERVICE'] == '1';
  final skip = serviceUri == null && !required
      ? 'needs flutter test --enable-vmservice to force garbage collection'
      : null;

  test('a forgotten session still answers the command it owes', () async {
    expect(serviceUri, isNotNull, reason: 'run with --enable-vmservice');
    final sandbox = Directory.systemTemp.createTempSync('kineto-session-gc-');
    final target = '${sandbox.path}${Platform.pathSeparator}film';
    try {
      // Queue enough work that it is still running when the session becomes
      // unreachable, then keep only the last answer.
      final (last,) = await _queueAndForget(target, commands: 50);
      await _forceFullGc(serviceUri!);

      await expectLater(
        last.timeout(const Duration(seconds: 30)),
        completes,
      );

      // Once nothing is owed, collection reclaims the session and releases
      // the project's writer lock.
      await _eventuallyOpens(target, serviceUri);
    } finally {
      sandbox.deleteSync(recursive: true);
    }
  }, skip: skip);
}

Future<(Future<void>,)> _queueAndForget(String target, {required int commands}) async {
  final session = await KinetoProjectSession.createTextProject(
    path: target,
    projectId: 'session_gc',
    title: 'Session GC',
    createdAt: '2026-09-30T00:00:00Z',
    language: 'en',
    sourceText: 'A filmmaker waits across the table.\n',
  );
  late Future<void> last;
  for (var index = 0; index < commands; index++) {
    last = session.generateShotCandidates(0);
  }
  return (last,);
}

Future<void> _eventuallyOpens(String target, Uri serviceUri) async {
  for (var attempt = 0; attempt < 20; attempt++) {
    await _forceFullGc(serviceUri);
    try {
      final reopened = await KinetoProjectSession.open(target);
      reopened.close();
      return;
    } on KinetoProjectException catch (error) {
      if (error.code != KinetoProjectException.busy) rethrow;
      await Future<void>.delayed(const Duration(milliseconds: 50));
    }
  }
  fail('the collected session never released the project');
}

Future<void> _forceFullGc(Uri serviceUri) async {
  final socket = await WebSocket.connect(serviceUri.toString());
  try {
    final isolateId = Service.getIsolateId(Isolate.current);
    socket.add(jsonEncode(<String, Object?>{
      'jsonrpc': '2.0',
      'id': 'gc',
      'method': 'getAllocationProfile',
      'params': <String, Object?>{'isolateId': isolateId, 'gc': true},
    }));
    await socket
        .map((event) => jsonDecode(event as String) as Map<String, Object?>)
        .firstWhere((message) => message['id'] == 'gc')
        .timeout(const Duration(seconds: 10));
  } finally {
    await socket.close();
  }
}
