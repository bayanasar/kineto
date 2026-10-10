import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/demo_workspace.dart';
import 'package:kineto/kineto_controls.dart';
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
  bool unresolved = false,
  VoidCallback? onResubmit,
  String? error,
  VoidCallback? onConsent,
  List<KinetoTake> takes = const <KinetoTake>[],
  List<List<String>> staleReasons = const <List<String>>[],
  ValueChanged<String>? onRefine,
}) =>
    MaterialApp(
      theme: WabTheme.materialTheme(lightTheme: false),
      home: DemoWorkspaceView(
        engineSnapshot: const KinetoEngineSnapshot(abiVersion: 19),
        imageProvider: provider,
        candidateMedia: media,
        candidateOrigins: origins,
        keyframesUnresolved: unresolved,
        onResubmitKeyframes: onResubmit,
        error: error,
        onGenerateDespiteIdentityDrift: onConsent,
        takes: takes,
        staleReasons: staleReasons,
        onRefine: onRefine,
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

  test('a connection\'s keyframe is named after the connection', () {
    expect(
      const KinetoCandidateOrigin(provider: 'connection.nano', model: 'gemini-3.1-flash-image').label,
      'nano · gemini-3.1-flash-image',
    );
    expect(const KinetoCandidateOrigin(provider: 'minimax', model: 'image-01').label, 'MiniMax image-01');
  });

  testWidgets('a take is chosen with the plain button, not the card\'s seal-red one', (tester) async {
    final sandbox = Directory.systemTemp.createTempSync('kineto-keyframes-');
    addTearDown(() => sandbox.deleteSync(recursive: true));
    var chosen = -1;

    await tester.pumpWidget(
      MaterialApp(
        theme: WabTheme.materialTheme(lightTheme: false),
        home: DemoWorkspaceView(
          engineSnapshot: const KinetoEngineSnapshot(abiVersion: 19),
          imageProvider: _minimax,
          candidateMedia: _keyframes(sandbox),
          shotSnapshot: _threeTakes,
          shotSnapshots: const <KinetoShotSnapshot>[_threeTakes, _empty],
          activeShotIndex: 0,
          onShotChanged: (_) {},
          onDirectionChanged: (_) {},
          onGenerate: () {},
          onSelect: (index) => chosen = index,
          onLock: () {},
          onReset: () {},
        ),
      ),
    );

    expect(find.byType(WabButton), findsNothing, reason: 'no textured material on the takes');
    for (final index in <int>[0, 1, 2]) {
      final button = tester.widget<KinetoButton>(find.byKey(ValueKey<String>('select-take-$index')));
      expect(button.primary, index == 1, reason: 'only the chosen take is gold');
    }
    await tester.ensureVisible(find.byKey(const ValueKey<String>('select-take-2')));
    await tester.tap(find.byKey(const ValueKey<String>('select-take-2')));
    expect(chosen, 2);
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
      find.textContaining('save one under Settings, Keys'),
      findsOneWidget,
    );
    final refused = find.text(r'Generate 3 keyframes · MiniMax image-01 · ≈ $0.0105');
    await tester.ensureVisible(refused);
    await tester.pumpAndSettle();
    await tester.tap(refused, warnIfMissed: false);
    expect(generated, 1, reason: 'no generation without a key');
  });

  testWidgets('an unresolved generation is asked for again only as an explicit risk', (tester) async {
    var generated = 0;
    var resubmitted = 0;
    await tester.pumpWidget(
      _workspace(
        shot: _empty,
        provider: _minimax,
        unresolved: true,
        onGenerate: () => generated++,
        onResubmit: () => resubmitted++,
      ),
    );
    expect(find.textContaining('may already have charged for keyframes'), findsOneWidget);
    final generate = find.text(r'Generate 3 keyframes · MiniMax image-01 · ≈ $0.0105');
    await tester.ensureVisible(generate);
    await tester.pumpAndSettle();
    await tester.tap(generate, warnIfMissed: false);
    final resubmit = find.text(r'Generate again · may pay twice · ≈ $0.0105');
    await tester.ensureVisible(resubmit);
    await tester.pumpAndSettle();
    await tester.tap(resubmit);
    expect((generated, resubmitted), (0, 1));
  });

  testWidgets('generating despite identity drift is offered only after the policy refused it', (tester) async {
    tester.view.physicalSize = const Size(1280, 2000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(_workspace(shot: _empty, provider: _minimax, error: 'The provider failed.'));
    expect(find.byKey(const ValueKey<String>('generate-despite-drift')), findsNothing);

    var consented = 0;
    await tester.pumpWidget(
      _workspace(
        shot: _empty,
        provider: _minimax,
        error: 'A character in this shot has no locked cast.',
        onConsent: () => consented++,
      ),
    );
    expect(find.textContaining('a different-looking person is acceptable'), findsOneWidget);
    final consent = find.byKey(const ValueKey<String>('generate-despite-drift'));
    await tester.ensureVisible(consent);
    await tester.tap(consent);
    await tester.pump();
    expect(consented, 1);
  });

  testWidgets('a shot is refined by a note, and earlier takes and stale reasons are shown', (tester) async {
    tester.view.physicalSize = const Size(1280, 2600);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final notes = <String>[];
    await tester.pumpWidget(
      _workspace(
        shot: _threeTakes,
        provider: _minimax,
        media: const <String?>[null, null, null],
        takes: const <KinetoTake>[
          KinetoTake(id: 'office_night_02_g0001_candidate_a', generation: 1, status: KinetoTakeStatus.superseded),
          KinetoTake(id: 'office_night_02_g0001_candidate_b', generation: 1, status: KinetoTakeStatus.superseded),
          KinetoTake(
            id: 'office_night_02_g0002_candidate_a',
            generation: 2,
            status: KinetoTakeStatus.candidate,
            refined: true,
          ),
        ],
        staleReasons: const <List<String>>[
          <String>['intent.emotion'],
          <String>[],
          <String>['intent.emotion', 'style_s0001'],
        ],
        onRefine: notes.add,
      ),
    );
    expect(find.text('Earlier takes'), findsOneWidget);
    expect(find.text('Generation 1 · a'), findsOneWidget);
    expect(find.text('Generation 1 · b'), findsOneWidget);
    expect(find.text('Take A no longer follows: intent.emotion'), findsOneWidget);
    expect(find.text('Take C no longer follows: intent.emotion, style_s0001'), findsOneWidget);
    expect(find.textContaining('Take B no longer follows'), findsNothing);

    final refine = find.byKey(const ValueKey<String>('refine-shot'));
    await tester.ensureVisible(refine);
    await tester.tap(refine);
    await tester.pumpAndSettle();
    await tester.enterText(find.byKey(const ValueKey<String>('refine-shot-note')), ' Warmer light. ');
    await tester.tap(find.byKey(const ValueKey<String>('refine-shot-confirm')));
    await tester.pumpAndSettle();
    expect(notes, <String>['Warmer light.']);
  });

  testWidgets('a selected take whose locks moved on is not offered for locking', (tester) async {
    var locks = 0;
    Widget workspace(List<List<String>> staleReasons) => MaterialApp(
          theme: WabTheme.materialTheme(lightTheme: false),
          home: DemoWorkspaceView(
            engineSnapshot: const KinetoEngineSnapshot(abiVersion: 19),
            imageProvider: _minimax,
            candidateMedia: const <String?>[null, null, null],
            staleReasons: staleReasons,
            shotSnapshot: _threeTakes,
            shotSnapshots: const <KinetoShotSnapshot>[_threeTakes, _empty],
            activeShotIndex: 0,
            onShotChanged: (_) {},
            onDirectionChanged: (_) {},
            onGenerate: () {},
            onSelect: (_) {},
            onLock: () => locks++,
            onReset: () {},
          ),
        );
    final lock = find.byKey(const ValueKey<String>('lock-selection'));

    await tester.pumpWidget(workspace(List<List<String>>.filled(3, const <String>['style_s0002'])));
    expect(find.text('Regenerate before locking'), findsOneWidget);
    await tester.ensureVisible(lock);
    await tester.tap(lock, warnIfMissed: false);
    expect(locks, 0);

    await tester.pumpWidget(workspace(List<List<String>>.filled(3, const <String>[])));
    expect(find.text('Lock selection'), findsOneWidget);
    await tester.tap(lock);
    expect(locks, 1);
  });
}
