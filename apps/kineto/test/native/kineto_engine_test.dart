import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/native/kineto_engine.dart';

void main() {
  test('bindings and bundled native library agree on the ABI revision', () {
    final engine = KinetoEngine.open();
    addTearDown(engine.close);
    expect(engine.snapshot.abiVersion, kinetoExpectedNativeAbiVersion);
  });

  test('Flutter loads the bundled Rust CodeAsset through typed FFI', () {
    final engine = KinetoEngine.open();
    addTearDown(engine.close);

    final snapshot = engine.snapshot;
    expect(snapshot.abiVersion, kinetoExpectedNativeAbiVersion);
  });

  test('explicit close is idempotent and native calls reject use-after-close', () {
    final engine = KinetoEngine.open();
    engine.close();
    engine.close();

    expect(() => engine.snapshot, throwsStateError);
  });
}
