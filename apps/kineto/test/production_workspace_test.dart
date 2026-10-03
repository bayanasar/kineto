import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/native/kineto_engine.dart';
import 'package:kineto/production_workspace.dart';
import 'package:wabisabi/wabisabi.dart';

const KinetoBible _bible = KinetoBible(
  revision: 2,
  setting: 'A city at night in the rain.',
  period: 'The present day.',
  visualRules: <String>['Practical light only.'],
  characters: <KinetoBibleEntry>[
    KinetoBibleEntry(
      kind: KinetoBibleEntryKind.character,
      id: 'filmmaker',
      name: 'Daniel',
      description: 'A documentary filmmaker.',
      appearance: 'Grey at the temples.',
      wardrobe: 'A worn overcoat.',
      forbiddenChanges: <String>['Never without the coat.'],
    ),
  ],
  locations: <KinetoBibleEntry>[
    KinetoBibleEntry(
      kind: KinetoBibleEntryKind.location,
      id: 'small_office',
      name: 'The office',
      description: 'A rented office.',
      appearance: 'One desk lamp.',
    ),
  ],
);

const KinetoProduction _drafted = KinetoProduction(
  bible: _bible,
  bibleRevisions: <KinetoRevision>[
    KinetoRevision(
      number: 1,
      status: KinetoRevisionStatus.superseded,
      origin: KinetoRevisionOrigin.generated,
      provider: 'kimi-code',
      model: 'agent-default',
    ),
    KinetoRevision(
      number: 2,
      status: KinetoRevisionStatus.candidate,
      origin: KinetoRevisionOrigin.edited,
      basedOn: 1,
    ),
  ],
  presets: <KinetoStylePreset>[
    KinetoStylePreset(id: 'film_realism', name: 'Realist film'),
    KinetoStylePreset(id: 'film_noir', name: 'Film noir'),
  ],
);

KinetoCastImage _image(String id, KinetoCastImageKind kind, {KinetoChoiceStatus? status, String? from}) =>
    KinetoCastImage(
      id: id,
      kind: kind,
      mediaPath: '/nonexistent/$id.jpg',
      status: status,
      identityReference: from,
    );

final KinetoProduction _casting = KinetoProduction(
  bible: _bible,
  bibleLocked: 2,
  presets: _drafted.presets,
  samples: const <KinetoStyleSample>[
    KinetoStyleSample(
      id: 'style_s0001',
      preset: 'film_noir',
      sceneId: 'office_night',
      status: KinetoChoiceStatus.selected,
      mediaPath: '/nonexistent/style_s0001.jpg',
    ),
    KinetoStyleSample(
      id: 'style_s0002',
      preset: 'film_realism',
      sceneId: 'office_night',
      status: KinetoChoiceStatus.candidate,
      mediaPath: '/nonexistent/style_s0002.jpg',
      note: 'Warmer.',
    ),
  ],
  style: const KinetoStyleLock(sampleId: 'style_s0001', preset: 'film_noir'),
  characters: <KinetoCastCharacter>[
    KinetoCastCharacter(
      id: 'filmmaker',
      name: 'Daniel',
      images: <KinetoCastImage>[
        _image('cast.filmmaker.0001', KinetoCastImageKind.portrait, status: KinetoChoiceStatus.candidate),
        _image('cast.filmmaker.0002', KinetoCastImageKind.portrait, status: KinetoChoiceStatus.selected),
        _image('cast.filmmaker.0003', KinetoCastImageKind.front, from: 'cast.filmmaker.0002'),
        _image('cast.filmmaker.0004', KinetoCastImageKind.fullBody, from: 'cast.filmmaker.0002'),
      ],
    ),
  ],
);

const ProductionPrices _minimax = ProductionPrices(
  sample: KinetoImageCost(micros: 3500, images: 1),
  portrait: KinetoImageCost(micros: 3500, images: 1),
  pack: KinetoImageCost(micros: 24500, images: 7),
);

const KinetoLanguageModel _kimiCode =
    KinetoLanguageModel(kind: KinetoLanguageModelKind.kimiCode, ready: true, model: 'agent-default');

final class _Calls {
  final List<String> log = <String>[];
  KinetoBibleEntryEdit? saved;
}

Future<_Calls> _pump(
  WidgetTester tester, {
  KinetoProduction production = _drafted,
  ProductionPrices prices = _minimax,
  bool screenplayLocked = true,
  bool working = false,
  String? editing,
  String? error,
  bool offerResubmit = false,
}) async {
  tester.view.physicalSize = const Size(1280, 3200);
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  final calls = _Calls();
  await tester.pumpWidget(
    MaterialApp(
      theme: WabTheme.materialTheme(lightTheme: false),
      home: ProductionWorkspaceView(
        production: production,
        model: _kimiCode,
        prices: prices,
        screenplayLocked: screenplayLocked,
        working: working,
        workingLabel: 'Casting',
        editingEntry: editing,
        error: error,
        onResubmit: offerResubmit ? () => calls.log.add('resubmit') : null,
        onWriteBible: () => calls.log.add('write'),
        onEditEntry: (entry) => calls.log.add('edit $entry'),
        onSaveEntry: (edit) => calls.saved = edit,
        onLockBible: (revision) => calls.log.add('lock bible $revision'),
        onSample: (preset, note) => calls.log.add('sample $preset $note'),
        onRefine: (sample, note) => calls.log.add('refine $sample $note'),
        onSelectStyle: (sample) => calls.log.add('select $sample'),
        onRejectStyle: (sample) => calls.log.add('reject $sample'),
        onLockStyle: (sample) => calls.log.add('lock $sample'),
        onAudition: (character, count) => calls.log.add('audition $character $count'),
        onMakePack: (character, remake) =>
            calls.log.add('pack $character ${remake.map((kind) => kind.wireName).join(',')}'),
        onSelectPortrait: (character, portrait) => calls.log.add('select $portrait'),
        onRejectPortrait: (character, portrait) => calls.log.add('reject $portrait'),
        onLockCast: (character) => calls.log.add('cast $character'),
      ),
    ),
  );
  return calls;
}

Future<void> _tap(WidgetTester tester, Finder finder) async {
  await tester.ensureVisible(finder);
  await tester.tap(finder, warnIfMissed: false);
  await tester.pump();
}

ValueKey<String> _key(String key) => ValueKey<String>(key);

void main() {
  testWidgets('the bible reads as a bible, and its draft is locked or edited', (tester) async {
    final calls = await _pump(tester);
    expect(find.text('Bible · revision 2'), findsOneWidget);
    expect(find.text('Draft'), findsOneWidget);
    expect(find.text('Daniel · character'), findsOneWidget);
    expect(find.text('Never changes: Never without the coat.'), findsOneWidget);
    expect(find.text('· Practical light only.'), findsOneWidget);
    expect(find.text('Revision 2 · edited from revision 1 · draft'), findsOneWidget);
    expect(find.text('Kimi Code · cost unknown: billed by the Kimi Code subscription'), findsOneWidget);
    expect(find.text('Lock a style first; every portrait follows it.'), findsNothing,
        reason: 'no character is worth casting before the bible names one');

    await _tap(tester, find.byKey(_key('lock-bible')));
    await _tap(tester, find.byKey(_key('edit-character-filmmaker')));
    await _tap(tester, find.byKey(_key('write-bible')));
    expect(calls.log, <String>['lock bible 2', 'edit character-filmmaker', 'write']);
  });

  testWidgets('without a locked screenplay the bible cannot be written', (tester) async {
    final calls = await _pump(tester, production: const KinetoProduction(), screenplayLocked: false);
    expect(find.text('Lock a screenplay first; the bible is written from it.'), findsOneWidget);
    await _tap(tester, find.byKey(_key('write-bible')));
    expect(calls.log, isEmpty);
  });

  testWidgets('an entry is edited in place and saved whole', (tester) async {
    final calls = await _pump(tester, editing: 'character-filmmaker');
    expect(find.text('Editing Daniel'), findsOneWidget);
    await tester.enterText(find.byKey(_key('entry-filmmaker-appearance')), 'Grey at the temples, a scar.');
    await tester.enterText(
      find.byKey(_key('entry-filmmaker-forbidden')),
      'Never without the coat.\n\n  The scar is on the left. ',
    );
    await _tap(tester, find.byKey(_key('save-entry-filmmaker')));
    final saved = calls.saved!;
    expect(saved.kind, KinetoBibleEntryKind.character);
    expect(saved.id, 'filmmaker');
    expect(saved.appearance, 'Grey at the temples, a scar.');
    expect(saved.wardrobe, 'A worn overcoat.');
    expect(saved.forbiddenChanges, <String>['Never without the coat.', 'The scar is on the left.']);

    // An entry left without what it needs is not saved.
    calls.saved = null;
    await tester.enterText(find.byKey(_key('entry-filmmaker-wardrobe')), ' ');
    await _tap(tester, find.byKey(_key('save-entry-filmmaker')));
    expect(calls.saved, isNull);
    expect(find.text('A character needs a description, an appearance and a wardrobe.'), findsOneWidget);
  });

  testWidgets('a style is sampled at its stated cost, then chosen, refined and locked', (tester) async {
    var calls = await _pump(tester);
    expect(find.text('Sample · 1 image ≈ \$0.0035'), findsOneWidget);
    await _tap(tester, find.byKey(_key('preset-film_noir')));
    await tester.enterText(find.byKey(_key('style-note')), 'Harder shadows.');
    await _tap(tester, find.byKey(_key('sample-style')));
    expect(calls.log, <String>['sample film_noir Harder shadows.']);

    calls = await _pump(tester, production: _casting);
    expect(find.text('Locked · film_noir'), findsOneWidget);
    expect(find.text('style_s0001 · film_noir · locked'), findsOneWidget);
    expect(find.byKey(_key('lock-style_s0001')), findsNothing, reason: 'it is already locked');
    expect(find.byKey(_key('reject-style_s0001')), findsNothing, reason: 'the locked sample stays');
    await _tap(tester, find.byKey(_key('select-style_s0002')));
    await _tap(tester, find.byKey(_key('reject-style_s0002')));
    await _tap(tester, find.byKey(_key('refine-style_s0002')));
    await tester.enterText(find.byKey(_key('refine-note')), 'Less grain.');
    await tester.tap(find.byKey(_key('refine-confirm')));
    await tester.pumpAndSettle();
    expect(calls.log, <String>['select style_s0002', 'reject style_s0002', 'refine style_s0002 Less grain.']);
  });

  testWidgets('without MiniMax nothing paid can be asked for, and the director is told why', (tester) async {
    final calls = await _pump(tester, production: _casting, prices: const ProductionPrices());
    expect(find.textContaining('Style samples need MiniMax'), findsOneWidget);
    expect(find.text('Sample · cost unknown'), findsOneWidget);
    for (final key in <String>['sample-style', 'refine-style_s0002', 'audition-filmmaker', 'pack-filmmaker']) {
      await _tap(tester, find.byKey(_key(key)));
    }
    expect(calls.log, isEmpty);
  });

  testWidgets('a character is cast from a chosen portrait and its complete pack', (tester) async {
    var calls = await _pump(tester, production: _casting);
    expect(find.text('Portrait chosen'), findsOneWidget);
    // Three wanted, two kept: one more is made.
    expect(find.text('Audition · 1 image ≈ \$0.0035'), findsOneWidget);
    expect(find.text('Reference pack of cast.filmmaker.0002'), findsOneWidget);
    // Seven in a pack, two made: five more.
    expect(find.text('Make the pack · 5 images ≈ \$0.0175'), findsOneWidget);
    expect(find.byKey(_key('lock-cast-filmmaker')), findsNothing, reason: 'the pack is not complete');
    await _tap(tester, find.byKey(_key('audition-filmmaker')));
    await _tap(tester, find.byKey(_key('select-cast.filmmaker.0001')));
    await _tap(tester, find.byKey(_key('reject-cast.filmmaker.0001')));
    await _tap(tester, find.byKey(_key('pack-filmmaker')));
    // A remade reference is paid: its price is shown first, and nothing is
    // asked for until the director confirms it.
    await _tap(tester, find.byKey(_key('remake-filmmaker-full_body')));
    await tester.pumpAndSettle();
    expect(find.text('Remake the full body reference of Daniel?'), findsOneWidget);
    expect(find.text('Remake · 1 image ≈ \$0.0035'), findsOneWidget);
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
    expect(calls.log.last, 'pack filmmaker ');
    await _tap(tester, find.byKey(_key('remake-filmmaker-full_body')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(_key('remake-confirm')));
    await tester.pumpAndSettle();
    expect(calls.log, <String>[
      'audition filmmaker 3',
      'select cast.filmmaker.0001',
      'reject cast.filmmaker.0001',
      'pack filmmaker ',
      'pack filmmaker full_body',
    ]);

    final selected = _casting.characters.single.images[1];
    final complete = KinetoProduction(
      bible: _bible,
      style: _casting.style,
      characters: <KinetoCastCharacter>[
        KinetoCastCharacter(
          id: 'filmmaker',
          name: 'Daniel',
          images: <KinetoCastImage>[
            selected,
            for (final kind in KinetoCastImageKind.values.skip(1))
              _image('ref-${kind.wireName}', kind, from: selected.id),
          ],
        ),
      ],
    );
    calls = await _pump(tester, production: complete);
    expect(find.byKey(_key('pack-filmmaker')), findsNothing);
    await _tap(tester, find.byKey(_key('lock-cast-filmmaker')));
    expect(calls.log, <String>['cast filmmaker']);
  });

  testWidgets('a cast says when its bible entry or the style changed since', (tester) async {
    final images = _casting.characters.single.images;
    await _pump(
      tester,
      production: KinetoProduction(
        bible: _bible,
        style: _casting.style,
        characters: <KinetoCastCharacter>[
          KinetoCastCharacter(
            id: 'filmmaker',
            name: 'Daniel',
            images: images,
            lockedPortrait: 'cast.filmmaker.0002',
            stale: const <String>['bible_character.filmmaker'],
            unresolved: true,
          ),
        ],
      ),
    );
    expect(find.text('Cast · changed since: bible_character.filmmaker'), findsOneWidget);
    expect(find.text('cast.filmmaker.0002 · cast'), findsOneWidget);
    expect(find.byKey(_key('reject-cast.filmmaker.0002')), findsNothing, reason: 'the cast portrait stays');
    expect(find.textContaining('MiniMax may already have charged for it'), findsOneWidget);
  });

  testWidgets('while something is made every change waits', (tester) async {
    final calls = await _pump(tester, production: _casting, working: true);
    expect(find.text('Casting'), findsWidgets);
    for (final key in <String>[
      'write-bible',
      'edit-character-filmmaker',
      'sample-style',
      'select-style_s0002',
      'audition-filmmaker',
      'select-cast.filmmaker.0001',
      'pack-filmmaker',
    ]) {
      await _tap(tester, find.byKey(_key(key)));
    }
    expect(calls.log, isEmpty);
  });

  testWidgets('asking again at the risk of paying twice is offered only after that refusal', (tester) async {
    var calls = await _pump(tester, error: 'Lock a style before casting');
    expect(find.byKey(_key('resubmit-paid')), findsNothing);
    calls = await _pump(tester, error: 'A paid call may already have been accepted.', offerResubmit: true);
    expect(find.text('Ask again · may pay twice'), findsOneWidget);
    await _tap(tester, find.byKey(_key('resubmit-paid')));
    expect(calls.log, <String>['resubmit']);
  });
}
