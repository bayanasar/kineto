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
    List<String?>? keyframes,
    List<KinetoVideoTake> videos = const <KinetoVideoTake>[],
    void Function(int, String?, String?)? onGenerateVideos,
    KinetoVideoEstimate? estimate,
    List<String> failures = const <String>[],
    int activeShot = 0,
    VoidCallback? onResubmitVideo,
    ValueChanged<int>? onSelectVideo,
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
          candidateMedia: keyframes ?? media,
          videoTakes: videos,
          videoEstimate: estimate,
          videoFailures: failures,
          shotSnapshot: shot,
          shotSnapshots: <KinetoShotSnapshot>[shot, shot],
          activeShotIndex: activeShot,
          onShotChanged: (_) {},
          onDirectionChanged: (_) {},
          onGenerate: () {},
          onSelect: (_) {},
          onLock: () {},
          onReset: () {},
          onGenerateVideos: onGenerateVideos,
          onResubmitVideo: onResubmitVideo,
          onSelectVideo: onSelectVideo,
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
    await tester.pumpAndSettle();
  }

  KinetoVideoTake take(
    int generation,
    int candidate,
    KinetoTakeStatus status, {
    List<String> notes = const <String>[],
    List<String> stale = const <String>[],
  }) =>
      KinetoVideoTake(
        id: 'shot_001_v000${generation}_candidate_${String.fromCharCode(0x61 + candidate)}',
        generation: generation,
        candidate: candidate,
        status: status,
        mediaPath: '/project/videos/g$generation$candidate.mp4',
        provider: 'minimax-video',
        model: 'MiniMax-Hailuo-02',
        notes: notes,
        stale: stale,
      );

  testWidgets('a locked keyframe offers priced takes, as many as asked', (tester) async {
    final asked = <(int, String?, String?)>[];
    await pump(
      tester,
      state: KinetoVideoState.none,
      onGenerateVideos: (count, note, branch) => asked.add((count, note, branch)),
    );
    expect(find.text('No video'), findsOneWidget);
    expect(find.text(r'Generate · 1 take · 6 s · ≈ $0.28'), findsOneWidget);
    await tap(tester, find.byKey(const ValueKey<String>('video-count')));
    await tester.tap(find.text('3 takes').last);
    await tester.pumpAndSettle();
    await tap(tester, find.text(r'Generate · 3 takes · 6 s · ≈ $0.84'));
    expect(asked, <(int, String?, String?)>[(3, null, null)]);
    expect(find.byKey(const ValueKey<String>('refine-video')), findsNothing, reason: 'nothing to refine yet');
  });

  testWidgets('a shot is priced at the clip length its plan asks for', (tester) async {
    final asked = <int>[];
    await pump(
      tester,
      state: KinetoVideoState.none,
      estimate: const KinetoVideoEstimate(clipSeconds: 10, costMicros: 560000),
      onGenerateVideos: (count, _, _) => asked.add(count),
    );
    expect(find.text(r'Generate · 1 take · 10 s · ≈ $0.56'), findsOneWidget);
    await tap(tester, find.byKey(const ValueKey<String>('video-count')));
    await tester.tap(find.text('2 takes').last);
    await tester.pumpAndSettle();
    await tap(tester, find.text(r'Generate · 2 takes · 10 s · ≈ $1.12'));
    expect(asked, <int>[2]);
  });

  testWidgets('a take that ended without a video is still said after other work', (tester) async {
    await pump(
      tester,
      state: KinetoVideoState.ready,
      videos: <KinetoVideoTake>[take(1, 0, KinetoTakeStatus.candidate)],
      failures: const <String>['The provider refused or failed'],
    );
    expect(find.text('A take did not arrive: The provider refused or failed.'), findsOneWidget);
    expect(find.text('Generation 1 · take a'), findsOneWidget);
  });

  testWidgets('another shot starts from one take again', (tester) async {
    await pump(tester, state: KinetoVideoState.none);
    await tap(tester, find.byKey(const ValueKey<String>('video-count')));
    await tester.tap(find.text('3 takes').last);
    await tester.pumpAndSettle();
    expect(find.text(r'Generate · 3 takes · 6 s · ≈ $0.84'), findsOneWidget);
    await pump(tester, state: KinetoVideoState.none, activeShot: 1);
    expect(find.text(r'Generate · 1 take · 6 s · ≈ $0.28'), findsOneWidget);
  });

  testWidgets('a video being generated can not be asked for again', (tester) async {
    var asked = 0;
    await pump(
      tester,
      state: KinetoVideoState.generating,
      onGenerateVideos: (_, _, _) => asked++,
    );
    expect(find.text('Generating'), findsWidgets);
    expect(find.textContaining('you can keep working'), findsOneWidget);
    final generate = find.byKey(const ValueKey<String>('generate-videos'));
    await tester.ensureVisible(generate);
    await tester.tap(generate, warnIfMissed: false);
    expect(asked, 0);
  });

  testWidgets('an unresolved submission is sent again only as an explicit risk', (tester) async {
    var generated = 0;
    var resubmitted = 0;
    await pump(
      tester,
      state: KinetoVideoState.unresolved,
      onGenerateVideos: (_, _, _) => generated++,
      onResubmitVideo: () => resubmitted++,
    );
    expect(find.text('Unresolved'), findsOneWidget);
    expect(find.textContaining('may already have'), findsOneWidget);
    expect(find.byKey(const ValueKey<String>('generate-videos')), findsNothing);
    await tap(tester, find.text(r'Submit again · may pay twice · ≈ $0.28'));
    expect((generated, resubmitted), (0, 1));
  });

  testWidgets('the latest takes are compared, one is chosen and locked, and earlier ones branched from', (
    tester,
  ) async {
    final asked = <(int, String?, String?)>[];
    final chosen = <int>[];
    final opened = <String>[];
    var locked = 0;
    Future<void> show(List<KinetoVideoTake> videos) => pump(
          tester,
          state: KinetoVideoState.ready,
          videos: videos,
          onGenerateVideos: (count, note, branch) => asked.add((count, note, branch)),
          onSelectVideo: chosen.add,
          onLockVideo: () => locked++,
          onOpenVideo: opened.add,
        );
    final earlier = take(1, 0, KinetoTakeStatus.superseded, notes: <String>['Slower.']);
    await show(<KinetoVideoTake>[
      earlier,
      take(2, 0, KinetoTakeStatus.candidate),
      take(2, 1, KinetoTakeStatus.candidate),
    ]);
    expect(find.text('Generation 2 · take a'), findsOneWidget);
    expect(find.text('Generation 2 · take b'), findsOneWidget);
    // Two takes, none chosen: nothing to lock yet.
    expect(find.byKey(const ValueKey<String>('lock-video')), findsNothing);
    await tap(tester, find.byKey(const ValueKey<String>('choose-shot_001_v0002_candidate_b')));
    expect(chosen, <int>[1]);
    await tap(tester, find.byKey(const ValueKey<String>('open-shot_001_v0002_candidate_a')));
    expect(opened, <String>['/project/videos/g20.mp4']);

    await show(<KinetoVideoTake>[
      earlier,
      take(2, 0, KinetoTakeStatus.candidate),
      take(2, 1, KinetoTakeStatus.selected),
    ]);
    expect(find.text('Generation 2 · take b · chosen'), findsOneWidget);
    await tap(tester, find.text('Lock take b'));
    expect(locked, 1);

    // A note refines the latest takes; a branch keeps an earlier take's.
    await tap(tester, find.byKey(const ValueKey<String>('refine-video')));
    await tester.enterText(find.byKey(const ValueKey<String>('video-note')), 'Hold on his eyes.');
    await tap(tester, find.byKey(const ValueKey<String>('video-note-confirm')));
    expect(find.text('Notes: Slower.'), findsOneWidget);
    await tap(tester, find.byKey(const ValueKey<String>('branch-shot_001_v0001_candidate_a')));
    await tester.enterText(find.byKey(const ValueKey<String>('video-note')), 'Closer');
    await tap(tester, find.byKey(const ValueKey<String>('video-note-confirm')));
    expect(asked, <(int, String?, String?)>[
      (1, 'Hold on his eyes.', null),
      (1, 'Closer', 'shot_001_v0001_candidate_a'),
    ]);
  });

  testWidgets('a take that no longer follows the shot says why and is not locked', (tester) async {
    await pump(
      tester,
      state: KinetoVideoState.stale,
      videos: <KinetoVideoTake>[
        take(1, 0, KinetoTakeStatus.candidate, stale: <String>['intent.emotion']),
      ],
      onLockVideo: () => fail('a stale take is not locked'),
    );
    expect(find.text('No longer follows: intent.emotion'), findsOneWidget);
    expect(find.byKey(const ValueKey<String>('lock-video')), findsNothing);
    expect(find.text(r'Generate again · 1 take · 6 s · ≈ $0.28'), findsOneWidget);
  });

  testWidgets('a locked video offers no regeneration', (tester) async {
    await pump(
      tester,
      state: KinetoVideoState.locked,
      videos: <KinetoVideoTake>[
        take(1, 0, KinetoTakeStatus.candidate),
        take(1, 1, KinetoTakeStatus.locked),
      ],
    );
    expect(find.text('Locked'), findsWidgets);
    expect(find.text('Generation 1 · take b · locked'), findsOneWidget);
    expect(find.byKey(const ValueKey<String>('generate-videos')), findsNothing);
    expect(find.byKey(const ValueKey<String>('lock-video')), findsNothing);
    expect(find.textContaining('Choose'), findsNothing);
  });

  testWidgets('the demo makes free clips from a keyframe without an image', (tester) async {
    final asked = <int>[];
    await pump(
      tester,
      state: KinetoVideoState.none,
      provider: _demo,
      keyframes: const <String?>[null, null, null],
      onGenerateVideos: (count, _, _) => asked.add(count),
    );
    expect(find.textContaining('placeholder clips'), findsOneWidget);
    await tap(tester, find.text('Generate · 1 take · 2 s · free'));
    expect(asked, <int>[1]);
  });

  testWidgets('MiniMax starts only from an image, and only with its key', (tester) async {
    var asked = 0;
    await pump(
      tester,
      state: KinetoVideoState.none,
      keyframes: const <String?>[null, null, null],
      onGenerateVideos: (_, _, _) => asked++,
    );
    expect(find.textContaining('has no image to start a video from'), findsOneWidget);
    final generate = find.byKey(const ValueKey<String>('generate-videos'));
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
    expect(find.byKey(const ValueKey<String>('generate-videos')), findsNothing);
  });
}
