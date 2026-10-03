import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/demo_workspace.dart';
import 'package:kineto/native/kineto_engine.dart';
import 'package:wabisabi/wabisabi.dart';

// A 1x1 PNG, so poster and keyframes point at a real image file.
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

const KinetoVideoProvider _hailuo = KinetoVideoProvider(
  kind: KinetoVideoProviderKind.minimax,
  ready: true,
  clipCostMicros: 280000,
  clipSeconds: 6,
);

const KinetoVideoProvider _demo = KinetoVideoProvider(
  kind: KinetoVideoProviderKind.localDemo,
  ready: true,
  clipCostMicros: 0,
  clipSeconds: 2,
);

KinetoShotSnapshot _locked(KinetoVideoState video) => KinetoShotSnapshot(
      direction: KinetoShotDirection.reaction,
      generated: true,
      stale: false,
      selectedIndex: 1,
      locked: true,
      candidateCount: 3,
      generation: 1,
      supersededCount: 0,
      videoState: video,
    );

void main() {
  late Directory sandbox;
  late List<String?> media;

  setUp(() {
    final binding = TestWidgetsFlutterBinding.ensureInitialized();
    binding.platformDispatcher.views.first.physicalSize = const Size(1280, 2400);
    binding.platformDispatcher.views.first.devicePixelRatio = 1.0;
    sandbox = Directory.systemTemp.createTempSync('kineto-video-panel-');
    media = <String?>[
      for (final take in <String>['a', 'b', 'c'])
        (File('${sandbox.path}${Platform.pathSeparator}$take.png')..writeAsBytesSync(_png))
            .path,
    ];
  });

  tearDown(() => sandbox.deleteSync(recursive: true));

  Future<void> pump(
    WidgetTester tester, {
    required KinetoVideoState state,
    KinetoVideoProvider provider = _hailuo,
    List<String?>? takes,
    String? video,
    VoidCallback? onGenerateVideo,
    VoidCallback? onResubmitVideo,
    VoidCallback? onLockVideo,
    ValueChanged<String>? onOpenVideo,
  }) async {
    final shot = _locked(state);
    await tester.pumpWidget(
      MaterialApp(
        theme: WabTheme.materialTheme(lightTheme: false),
        home: DemoWorkspaceView(
          engineSnapshot: const KinetoEngineSnapshot(abiVersion: 20),
          imageProvider: _minimax,
          videoProvider: provider,
          candidateMedia: takes ?? media,
          videoMedia: video,
          shotSnapshot: shot,
          shotSnapshots: <KinetoShotSnapshot>[shot, shot],
          activeShotIndex: 0,
          onShotChanged: (_) {},
          onDirectionChanged: (_) {},
          onGenerate: () {},
          onSelect: (_) {},
          onLock: () {},
          onReset: () {},
          onGenerateVideo: onGenerateVideo,
          onResubmitVideo: onResubmitVideo,
          onLockVideo: onLockVideo,
          onOpenVideo: onOpenVideo,
        ),
      ),
    );
  }

  Future<void> tap(WidgetTester tester, Finder finder) async {
    await tester.ensureVisible(finder);
    await tester.pumpAndSettle();
    await tester.tap(finder);
  }

  testWidgets('a locked keyframe offers a priced video', (tester) async {
    var asked = 0;
    await pump(tester, state: KinetoVideoState.none, onGenerateVideo: () => asked++);
    expect(find.text('No video'), findsOneWidget);
    await tap(tester, find.text(r'Generate video · 6 s · ≈ $0.28'));
    expect(asked, 1);
  });

  testWidgets('a video being generated can not be asked for again', (tester) async {
    var asked = 0;
    await pump(tester, state: KinetoVideoState.generating, onGenerateVideo: () => asked++);
    expect(find.text('Generating'), findsWidgets);
    expect(find.textContaining('you can keep working'), findsOneWidget);
    final regenerate = find.text(r'Regenerate video · ≈ $0.28');
    await tester.ensureVisible(regenerate);
    await tester.pumpAndSettle();
    await tester.tap(regenerate, warnIfMissed: false);
    expect(asked, 0);
  });

  testWidgets('an unresolved submission is sent again only as an explicit risk', (tester) async {
    var generated = 0;
    var resubmitted = 0;
    await pump(
      tester,
      state: KinetoVideoState.unresolved,
      onGenerateVideo: () => generated++,
      onResubmitVideo: () => resubmitted++,
    );
    expect(find.text('Unresolved'), findsOneWidget);
    expect(find.textContaining('may already have accepted and charged for it'), findsOneWidget);
    expect(find.text(r'Regenerate video · ≈ $0.28'), findsNothing);
    await tap(tester, find.text(r'Submit again · may pay twice · ≈ $0.28'));
    expect((generated, resubmitted), (0, 1));
  });

  testWidgets('a ready video opens, locks, and shows its first frame', (tester) async {
    String? opened;
    var locked = 0;
    await pump(
      tester,
      state: KinetoVideoState.ready,
      video: '/project/videos/shot_001_v0001.mp4',
      onOpenVideo: (path) => opened = path,
      onLockVideo: () => locked++,
    );
    // Three takes and the poster from the locked take.
    expect(find.byType(Image), findsNWidgets(4));
    await tap(tester, find.text('Open video'));
    expect(opened, '/project/videos/shot_001_v0001.mp4');
    await tap(tester, find.text('Lock video'));
    expect(locked, 1);
  });

  testWidgets('a locked video offers no regeneration', (tester) async {
    await pump(
      tester,
      state: KinetoVideoState.locked,
      video: '/project/videos/shot_001_v0001.mp4',
    );
    expect(find.text('Locked'), findsWidgets);
    expect(find.textContaining('Regenerate video'), findsNothing);
    expect(find.text('Lock video'), findsNothing);
  });

  testWidgets('the demo makes a free clip from a keyframe without an image', (tester) async {
    var asked = 0;
    await pump(
      tester,
      state: KinetoVideoState.none,
      provider: _demo,
      takes: const <String?>[null, null, null],
      onGenerateVideo: () => asked++,
    );
    expect(find.textContaining('placeholder clip'), findsOneWidget);
    await tap(tester, find.text('Generate video · 2 s · free'));
    expect(asked, 1);
  });

  testWidgets('MiniMax starts only from an image, and only with its key', (tester) async {
    var asked = 0;
    await pump(
      tester,
      state: KinetoVideoState.none,
      takes: const <String?>[null, null, null],
      onGenerateVideo: () => asked++,
    );
    expect(find.textContaining('has no image to start a video from'), findsOneWidget);
    final generate = find.text(r'Generate video · 6 s · ≈ $0.28');
    await tester.ensureVisible(generate);
    await tester.tap(generate, warnIfMissed: false);
    expect(asked, 0);

    await pump(
      tester,
      state: KinetoVideoState.none,
      provider: const KinetoVideoProvider(
        kind: KinetoVideoProviderKind.minimax,
        ready: false,
        clipCostMicros: 280000,
        clipSeconds: 6,
      ),
    );
    expect(find.textContaining('has no key configured'), findsOneWidget);
    expect(find.textContaining('Generate video'), findsNothing);
  });
}
