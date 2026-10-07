import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/handoff_workspace.dart';
import 'package:kineto/native/kineto_engine.dart';
import 'package:wabisabi/wabisabi.dart';

const List<KinetoSceneProgress> _scenes = <KinetoSceneProgress>[
  KinetoSceneProgress(
    sceneId: 'office_night',
    complete: true,
    shots: <KinetoShotProgress>[
      KinetoShotProgress(shotId: 'office_night_01', keyframe: KinetoStage.locked, video: KinetoStage.locked),
    ],
  ),
  KinetoSceneProgress(
    sceneId: 'street_dawn',
    complete: false,
    shots: <KinetoShotProgress>[
      KinetoShotProgress(shotId: 'street_dawn_01', keyframe: KinetoStage.locked, video: KinetoStage.made),
      KinetoShotProgress(shotId: 'street_dawn_02', keyframe: KinetoStage.made, video: KinetoStage.missing),
    ],
  ),
];

const KinetoExportState _partly = KinetoExportState(
  ready: 1,
  missing: <KinetoExportMissing>[
    KinetoExportMissing(sceneId: 'street_dawn', shotId: 'street_dawn_01', reason: 'its video is not locked'),
    KinetoExportMissing(sceneId: 'street_dawn', shotId: 'street_dawn_02', reason: 'its keyframe is not locked'),
  ],
);

const KinetoStorage _storage = KinetoStorage(
  usageBytes: 12400000,
  recentTakes: 6,
  owners: <(String, int)>[('shot office_night/office_night_01', 12000000), ('style', 400000)],
);

final class _Calls {
  final List<String> log = <String>[];
}

Future<_Calls> _pump(
  WidgetTester tester, {
  KinetoExportState export = _partly,
  KinetoStorage storage = _storage,
}) async {
  tester.view.physicalSize = const Size(1280, 3200);
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  final calls = _Calls();
  await tester.pumpWidget(
    MaterialApp(
      theme: WabTheme.materialTheme(lightTheme: false),
      home: HandoffWorkspaceView(
        scenes: _scenes,
        storage: storage,
        export: export,
        exportFolder: '/films/exports',
        onPlanCollection: () => calls.log.add('plan'),
        onCollect: () => calls.log.add('collect'),
        onVerify: () => calls.log.add('verify'),
        onExport: (folder, name, {required partial, required replace}) =>
            calls.log.add('export $folder $name partial=$partial replace=$replace'),
        onOpenFolder: (path) => calls.log.add('open $path'),
      ),
    ),
  );
  return calls;
}

Future<void> _tap(WidgetTester tester, Finder finder) async {
  await tester.ensureVisible(finder);
  await tester.tap(finder, warnIfMissed: false);
  await tester.pumpAndSettle();
}

ValueKey<String> _key(String key) => ValueKey<String>(key);

void main() {
  testWidgets('each scene says which shots still need what', (tester) async {
    await _pump(tester);
    expect(find.text('Complete'), findsOneWidget);
    expect(find.text('In progress'), findsOneWidget);
    expect(find.text('Needs a take chosen and locked'), findsOneWidget);
    expect(find.text('Needs a locked keyframe'), findsOneWidget);
    expect(find.text('Video · to choose'), findsOneWidget);
  });

  testWidgets('a package of only the ready shots is made only when asked for', (tester) async {
    final calls = await _pump(tester);
    expect(find.text('1 of 3 shots ready'), findsOneWidget);
    expect(find.text('street_dawn_02: its keyframe is not locked.'), findsOneWidget);
    await _tap(tester, find.byKey(_key('export')));
    expect(calls.log, isEmpty);
    await _tap(tester, find.descendant(of: find.byKey(_key('export-partial')), matching: find.byType(Checkbox)));
    await tester.enterText(find.byKey(_key('export-name')), 'rough-cut');
    await _tap(tester, find.byKey(_key('export')));
    expect(calls.log, <String>['export /films/exports rough-cut partial=true replace=false']);
  });

  testWidgets('a complete package is exported, then its folder opened', (tester) async {
    final calls = await _pump(
      tester,
      export: const KinetoExportState(
        ready: 3,
        last: KinetoExported(path: '/films/exports/dailies', shots: 3, complete: true, bytes: 3100000, lines: 2),
      ),
    );
    expect(find.text('Ready'), findsOneWidget);
    expect(find.byKey(_key('export-partial')), findsNothing);
    expect(find.text('Wrote /films/exports/dailies · 3 shots · 2 dubbed lines · 3.1 MB'), findsOneWidget);
    await _tap(tester, find.descendant(of: find.byKey(_key('export-replace')), matching: find.byType(Checkbox)));
    await _tap(tester, find.text('Export 3 shots'));
    await _tap(tester, find.byKey(_key('open-package')));
    expect(calls.log, <String>[
      'export /films/exports dailies partial=false replace=true',
      'open /films/exports/dailies',
    ]);
  });

  testWidgets('the disk is shown, and media is let go only after a dry run and a confirmation', (tester) async {
    var calls = await _pump(tester);
    expect(find.textContaining('Media takes 12.4 MB'), findsOneWidget);
    expect(find.text('style · 400.0 KB'), findsOneWidget);
    expect(find.byKey(_key('collect')), findsNothing);
    await _tap(tester, find.byKey(_key('plan-collection')));
    await _tap(tester, find.byKey(_key('verify-media')));
    expect(calls.log, <String>['plan', 'verify']);

    calls = await _pump(
      tester,
      storage: const KinetoStorage(
        usageBytes: 12400000,
        recentTakes: 2,
        plan: KinetoCollection(
          items: <KinetoMediaItem>[
            KinetoMediaItem(
              artifactId: 'office_night_01_g0001_candidate_a',
              owner: 'shot office_night/office_night_01',
              reason: 'replaced or turned down, and older than the latest 2 takes',
            ),
          ],
          bytes: 1200000,
        ),
        mismatched: <KinetoMediaItem>[KinetoMediaItem(artifactId: 'style_s0001', owner: 'style')],
      ),
    );
    expect(find.textContaining('1 file can be let go, freeing 1.2 MB'), findsOneWidget);
    expect(find.text('style_s0001 (style) is not what its record says.'), findsOneWidget);
    await _tap(tester, find.byKey(_key('collect')));
    await _tap(tester, find.text('Cancel'));
    expect(calls.log, isEmpty);
    await _tap(tester, find.byKey(_key('collect')));
    await _tap(tester, find.byKey(_key('collect-confirm')));
    expect(calls.log, <String>['collect']);
  });
}
