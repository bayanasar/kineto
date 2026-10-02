import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/demo_workspace.dart';
import 'package:kineto/native/kineto_engine.dart';
import 'package:wabisabi/wabisabi.dart';

// A 1x1 PNG, so the keyframe path points at a real image file.
const List<int> _png = <int>[
  0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, //
  0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
  0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00,
  0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
  0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00,
  0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

const KinetoImageProvider _minimax = KinetoImageProvider(
  kind: KinetoImageProviderKind.minimax,
  ready: true,
  generationCostMicros: 10500,
);

const KinetoShotSnapshot _empty = KinetoShotSnapshot(
  direction: KinetoShotDirection.reaction,
  generated: false,
  stale: false,
  selectedIndex: null,
  locked: false,
  candidateCount: 0,
  generation: 0,
  supersededCount: 0,
);

Widget _workspace({
  required KinetoShotSnapshot shot,
  required KinetoImageProvider provider,
  List<String?> media = const <String?>[],
  List<KinetoCandidateOrigin> origins = const <KinetoCandidateOrigin>[],
  VoidCallback? onGenerate,
}) =>
    MaterialApp(
      theme: WabTheme.materialTheme(lightTheme: false),
      home: DemoWorkspaceView(
        engineSnapshot: const KinetoEngineSnapshot(abiVersion: 19),
        imageProvider: provider,
        candidateMedia: media,
        candidateOrigins: origins,
        shotSnapshot: shot,
        shotSnapshots: <KinetoShotSnapshot>[shot, _empty],
        activeShotIndex: 0,
        onShotChanged: (_) {},
        onDirectionChanged: (_) {},
        onGenerate: onGenerate ?? () {},
        onSelect: (_) {},
        onLock: () {},
        onReset: () {},
      ),
    );

const KinetoCandidateOrigin _madeByMinimax =
    KinetoCandidateOrigin(provider: 'minimax', model: 'image-01');

const KinetoShotSnapshot _threeTakes = KinetoShotSnapshot(
  direction: KinetoShotDirection.reaction,
  generated: true,
  stale: false,
  selectedIndex: 1,
  locked: false,
  candidateCount: 3,
  generation: 1,
  supersededCount: 0,
);

List<String?> _keyframes(Directory sandbox) => <String?>[
      for (final take in <String>['a', 'b', 'c'])
        (File('${sandbox.path}${Platform.pathSeparator}shot_001_g0001_candidate_$take.png')
              ..writeAsBytesSync(_png))
            .path,
    ];

void main() {
  setUp(() {
    final binding = TestWidgetsFlutterBinding.ensureInitialized();
    binding.platformDispatcher.views.first.physicalSize = const Size(1280, 1600);
    binding.platformDispatcher.views.first.devicePixelRatio = 1.0;
  });

  testWidgets('provider keyframes are shown as takes from their files', (tester) async {
    final sandbox = Directory.systemTemp.createTempSync('kineto-keyframes-');
    addTearDown(() => sandbox.deleteSync(recursive: true));

    await tester.pumpWidget(
      _workspace(
        shot: _threeTakes,
        provider: _minimax,
        media: _keyframes(sandbox),
        origins: List<KinetoCandidateOrigin>.filled(3, _madeByMinimax),
      ),
    );

    expect(find.byType(Image), findsNWidgets(3));
    for (final take in <String>['Take A', 'Take B', 'Take C']) {
      expect(find.text(take), findsOneWidget);
    }
    expect(
      find.text('MiniMax image-01 keyframe · shot_001_g0001_candidate_b.png'),
      findsOneWidget,
    );
    expect(find.text('Wide master'), findsNothing);
    expect(find.text(r'Regenerate · ≈ $0.0105'), findsOneWidget);
    expect(find.text('Selected'), findsWidgets);
  });

  testWidgets('keyframes name who made them, not the session provider', (tester) async {
    // Generated with MiniMax, then reopened with the default local demo.
    final sandbox = Directory.systemTemp.createTempSync('kineto-keyframes-');
    addTearDown(() => sandbox.deleteSync(recursive: true));

    await tester.pumpWidget(
      _workspace(
        shot: _threeTakes,
        provider: const KinetoImageProvider(
          kind: KinetoImageProviderKind.localDemo,
          ready: true,
          generationCostMicros: 0,
        ),
        media: _keyframes(sandbox),
        origins: const <KinetoCandidateOrigin>[
          _madeByMinimax,
          _madeByMinimax,
          KinetoCandidateOrigin(provider: '', model: ''),
        ],
      ),
    );

    expect(find.byType(Image), findsNWidgets(3));
    expect(
      find.text('MiniMax image-01 keyframe · shot_001_g0001_candidate_a.png'),
      findsOneWidget,
    );
    expect(
      find.text('MiniMax image-01 keyframe · shot_001_g0001_candidate_b.png'),
      findsOneWidget,
    );
    // A record that names no provider is not attributed to anyone.
    expect(find.text('Keyframe · shot_001_g0001_candidate_c.png'), findsOneWidget);
    expect(find.textContaining('Local demo keyframe'), findsNothing);
  });

  testWidgets('demo candidates keep their cards when there is no media', (tester) async {
    const generated = KinetoShotSnapshot(
      direction: KinetoShotDirection.reaction,
      generated: true,
      stale: false,
      selectedIndex: null,
      locked: false,
      candidateCount: 3,
      generation: 1,
      supersededCount: 0,
    );

    await tester.pumpWidget(
      _workspace(
        shot: generated,
        provider: const KinetoImageProvider(
          kind: KinetoImageProviderKind.localDemo,
          ready: true,
          generationCostMicros: 0,
        ),
        media: const <String?>[null, null, null],
      ),
    );

    expect(find.byType(Image), findsNothing);
    expect(find.text('Wide master'), findsOneWidget);
    expect(find.text('Regenerate'), findsOneWidget);
  });

  testWidgets('a paid provider names its cost and is refused without a key', (tester) async {
    var generated = 0;
    await tester.pumpWidget(
      _workspace(shot: _empty, provider: _minimax, onGenerate: () => generated++),
    );
    final label = find.text(r'Generate 3 keyframes · MiniMax image-01 · ≈ $0.0105');
    expect(label, findsOneWidget);
    await tester.ensureVisible(label);
    await tester.pumpAndSettle();
    await tester.tap(label);
    expect(generated, 1);

    await tester.pumpWidget(
      _workspace(
        shot: _empty,
        provider: const KinetoImageProvider(
          kind: KinetoImageProviderKind.minimax,
          ready: false,
          generationCostMicros: 10500,
        ),
        onGenerate: () => generated++,
      ),
    );
    expect(
      find.textContaining('KINETO_MINIMAX_API_KEY'),
      findsOneWidget,
    );
    final refused = find.text(r'Generate 3 keyframes · MiniMax image-01 · ≈ $0.0105');
    await tester.ensureVisible(refused);
    await tester.pumpAndSettle();
    await tester.tap(refused, warnIfMissed: false);
    expect(generated, 1, reason: 'no generation without a key');
  });
}
