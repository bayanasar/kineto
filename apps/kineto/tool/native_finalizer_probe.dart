import 'dart:async';
import 'dart:convert';
import 'dart:developer';
import 'dart:io';
import 'dart:isolate';

import 'package:kineto/native/kineto_engine.dart';

Future<void> main() async {
  final mode = Platform.environment['KINETO_OWNERSHIP_PROBE_MODE'];
  if (mode == 'finalizer') {
    _openWithoutClosing();
    return;
  }
  if (mode == 'runtime-finalizer') {
    _openWithoutClosing();
    await _waitForRuntimeFinalizer();
    return;
  }
  if (mode == 'explicit-close') {
    final engine = KinetoEngine.open();
    _checkAbi(engine);
    engine.close();
    engine.close();
    _expectClosed(engine);
    return;
  }
  throw StateError('Unknown KINETO_OWNERSHIP_PROBE_MODE: $mode');
}

void _openWithoutClosing() {
  final engine = KinetoEngine.open();
  _checkAbi(engine);
}

Future<void> _waitForRuntimeFinalizer() async {
  final markerPath = Platform.environment['KINETO_OWNERSHIP_PROBE_FILE'];
  if (markerPath == null || markerPath.isEmpty) {
    throw StateError('KINETO_OWNERSHIP_PROBE_FILE is required');
  }
  final marker = File(markerPath);
  for (var attempt = 0; attempt < 20; attempt++) {
    await _forceFullGc();
    await Future<void>.delayed(const Duration(milliseconds: 50));
    if (marker.existsSync() &&
        marker.readAsLinesSync().any((line) => line.trim() == 'destroy')) {
      stdout.writeln('runtime finalizer observed before process exit');
      return;
    }
  }
  throw StateError('Runtime GC did not finalize unreachable KinetoEngine');
}

Future<void> _forceFullGc() async {
  final info = await Service.getInfo();
  final socketUri = info.serverWebSocketUri;
  final isolateId = Service.getIsolateId(Isolate.current);
  if (socketUri == null || isolateId == null) {
    throw StateError(
      'runtime-finalizer mode requires the Dart VM service '
      '(run with --enable-vm-service=0)',
    );
  }

  final socket = await WebSocket.connect(socketUri.toString());
  try {
    const requestId = 'kineto-gc';
    socket.add(jsonEncode(<String, Object?>{
      'jsonrpc': '2.0',
      'id': requestId,
      'method': 'getAllocationProfile',
      'params': <String, Object?>{
        'isolateId': isolateId,
        'gc': true,
      },
    }));

    await socket
        .map((event) => jsonDecode(event as String) as Map<String, Object?>)
        .firstWhere((message) => message['id'] == requestId)
        .timeout(const Duration(seconds: 5));
  } finally {
    await socket.close();
  }
}

void _checkAbi(KinetoEngine engine) {
  final actual = engine.snapshot.abiVersion;
  if (actual != kinetoExpectedNativeAbiVersion) {
    throw StateError(
      'Native ABI mismatch in ownership probe: '
      'expected $kinetoExpectedNativeAbiVersion, got $actual',
    );
  }
}

void _expectClosed(KinetoEngine engine) {
  try {
    engine.snapshot;
  } on StateError {
    return;
  }
  throw StateError('Explicitly closed engine still accepts native access');
}
