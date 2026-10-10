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

  test('a provider that gives no price says its cost is unknown, not free', () {
    const priced = KinetoImageProvider(
      kind: KinetoImageProviderKind.connection,
      ready: true,
      generationCostMicros: 120000,
    );
    const unpriced = KinetoImageProvider(
      kind: KinetoImageProviderKind.connection,
      ready: true,
      generationCostMicros: 0,
      costKnown: false,
    );
    expect(priced.generationCost, r'≈ $0.12');
    expect(unpriced.generationCost, 'cost unknown');
  });
}
