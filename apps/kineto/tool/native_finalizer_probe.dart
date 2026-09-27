import 'dart:io';

import 'package:kineto/native/kineto_engine.dart';

void main() {
  final mode = Platform.environment['KINETO_OWNERSHIP_PROBE_MODE'];
  if (mode == 'finalizer') {
    _openWithoutClosing();
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
