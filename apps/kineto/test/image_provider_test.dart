import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/native/kineto_engine.dart';

void main() {
  test('generation cost reads as dollars without trailing zeros', () {
    String label(int micros) => KinetoImageProvider(
          kind: KinetoImageProviderKind.minimax,
          ready: true,
          generationCostMicros: micros,
        ).generationCostLabel;

    expect(label(0), r'$0');
    expect(label(10500), r'$0.0105');
    expect(label(1250000), r'$1.25');
    expect(label(3000000), r'$3');
  });
}
