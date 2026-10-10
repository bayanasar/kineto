import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:wabisabi/wabisabi.dart';

import 'kineto_controls.dart';
import 'kineto_work.dart';
import 'media_frame.dart';
import 'native/kineto_engine.dart';
import 'references_panel.dart';
import 'screenplay_workspace.dart';

/// What paid images cost with the session's image provider, as it declares.
final class ProductionPrices {
  const ProductionPrices({this.sample, this.portrait, this.provides});

  final KinetoImageCost? sample;
  final KinetoImageCost? portrait;

  /// Whether the session's image provider makes images, when known.
  final bool? provides;

  /// Whether the session's image provider makes images: one without a
  /// price (an agent on a subscription, a plugin that does not say) still
  /// does, and says its cost is unknown.
  bool get makesImages => provides ?? (sample != null || portrait != null);
}

/// What asking for [cost] costs, in words.
String imageCostLabel(KinetoImageCost? cost) {
  if (cost == null) return 'cost unknown';
  final images = cost.images == 1 ? '1 image' : '${cost.images} images';
  return '$images ≈ ${formatDollars(cost.micros)}';
}

/// [cost] for [count] images of the same price.
KinetoImageCost? timesImages(KinetoImageCost? cost, int count) => cost == null
    ? null
    : KinetoImageCost(micros: cost.micros ~/ cost.images * count, images: count);

/// How many portrait candidates an audition asks to have.
const List<int> auditionSizes = <int>[1, 2, 3, 4, 6];

/// Asks the director for an image in the system's own open dialog; the
/// only way a path outside the project reaches Kineto.
Future<String?> pickImageFile() async {
  final file = await openFile(
    acceptedTypeGroups: const <XTypeGroup>[
      XTypeGroup(label: 'Images', extensions: <String>['jpg', 'jpeg', 'png', 'webp']),
    ],
  );
  return file?.path;
}

class ProductionWorkspaceScreen extends StatefulWidget {
  const ProductionWorkspaceScreen({
    required this.work,
    required this.project,
    this.navigation,
    this.isDarkMode = true,
    this.onThemeToggle,
    this.pickImage = pickImageFile,
    this.loadServers = loadKinetoMcpServersInBackground,
    super.key,
  });

  final KinetoProjectSession project;

  /// Asks the director for an image to import; null when they chose none.
  final Future<String?> Function() pickImage;

  /// Reads the registered MCP servers and their tools, off the UI thread.
  final Future<List<KinetoMcpServer>> Function() loadServers;

  /// Switches to the other workspaces, shown above the bible.
  final Widget? navigation;
  final bool isDarkMode;
  final VoidCallback? onThemeToggle;

  /// The project's requests, which outlive this page.
  final KinetoWork work;

  @override
  State<ProductionWorkspaceScreen> createState() => _ProductionWorkspaceScreenState();
}

class _ProductionWorkspaceScreenState extends State<ProductionWorkspaceScreen> {
  late final KinetoLanguageModel _model;
  late final ProductionPrices _prices;
  late KinetoProduction _production;
  late bool _screenplayLocked;
  int? _bibleCost;
  static const String _page = 'production';
  int _finished = 0;

  // The project's requests outlive this page: what is under way, and how
  // the last one asked for here ended, come from them.
  bool get _working => widget.work.busy;
  String get _workingLabel => widget.work.running?.label ?? 'Working';
  String? get _error => widget.work.problemOn(_page);
  VoidCallback? get _resubmit => widget.work.retryOn(_page);

  String? _editingEntry;
  late List<KinetoReference> _references;
  List<KinetoMcpServer>? _servers;
  String? _serversProblem;
  bool _loadingTools = false;

  @override
  void initState() {
    super.initState();
    _finished = widget.work.finished;
    widget.work.addListener(_workChanged);
    _model = widget.project.languageModel;
    final images = widget.project.imageProvider;
    _prices = ProductionPrices(
      sample: widget.project.imageCost(KinetoImageCostKind.sample),
      portrait: widget.project.imageCost(KinetoImageCostKind.portrait),
      provides: images.ready && images.kind != KinetoImageProviderKind.localDemo,
    );
    _read();
  }

  void _read() {
    _production = widget.project.production;
    _references = widget.project.references;
    _screenplayLocked = widget.project.documents.screenplayLocked;
    _bibleCost = widget.project.documentCostMicros(KinetoDocumentKind.bible);
  }

  // The bible, the style and the cast are made on the project's worker; this
  // isolate only waits. One command at a time keeps the page honest about
  // what it shows.
  void _workChanged() {
    if (!mounted) return;
    setState(() {
      if (widget.work.finished != _finished) {
        _finished = widget.work.finished;
        _read();
      }
    });
  }

  @override
  void dispose() {
    widget.work.removeListener(_workChanged);
    super.dispose();
  }

  Future<void> _run(
    Future<void> Function() action,
    String label, {
    VoidCallback? onDone,
    Future<void> Function()? resubmit,
  }) async {
    final done = await widget.work.run(_page, label, action, retry: resubmit);
    if (done && mounted) onDone?.call();
  }

  Future<void> _loadTools() async {
    setState(() {
      _loadingTools = true;
      _serversProblem = null;
    });
    List<KinetoMcpServer>? servers;
    String? problem;
    try {
      servers = await widget.loadServers();
    } on KinetoProjectException catch (failure) {
      problem = failure.message;
    } on Object catch (failure) {
      problem = failure.toString();
    }
    if (!mounted) return;
    setState(() {
      _loadingTools = false;
      _servers = servers;
      _serversProblem = problem;
    });
  }

  /// Ask for an image, then for the director's description of its style,
  /// which prompts follow; the image goes to a provider that takes it.
  Future<void> _importStyle() async {
    if (_working) return;
    final path = await widget.pickImage();
    if (path == null || !mounted) return;
    final description = await showDialog<String>(
      context: context,
      builder: (context) => const TextPromptDialog(
        title: 'Describe this style',
        width: 520,
        fieldKey: 'import-style-description',
        minLines: 2,
        maxLines: 5,
        hint: 'What every image should follow, such as "teal shadows, amber practicals, heavy grain"',
        confirmKey: 'import-style-confirm',
        confirmLabel: 'Import · free',
      ),
    );
    if (description == null || description.trim().isEmpty || !mounted) return;
    await _run(() => widget.project.importStyle(path, description.trim()), 'Importing the style');
  }

  Future<void> _importPortrait(String character) async {
    if (_working) return;
    final path = await widget.pickImage();
    if (path == null || !mounted) return;
    await _run(() => widget.project.importPortrait(character, path), 'Importing the portrait');
  }

  @override
  Widget build(BuildContext context) {
    final project = widget.project;
    return ProductionWorkspaceView(
      references: _references,
      servers: _servers,
      serversProblem: _serversProblem,
      loadingTools: _loadingTools,
      onLoadTools: _loadTools,
      onAskReference: (server, tool, arguments, targets) => _run(
        () => project.askReference(server: server, tool: tool, arguments: arguments, attachedTo: targets),
        'Asking $server for $tool',
      ),
      onEditReference: (reference, text) => _run(() => project.editReference(reference, text), 'Saving'),
      onAttachReference: (reference, target, attach) =>
          _run(() => project.attachReference(reference, target, attach: attach), 'Saving'),
      onRemoveReference: (reference) => _run(() => project.removeReference(reference), 'Removing'),
      onImportPortrait: _importPortrait,
      onImportStyle: _importStyle,
      production: _production,
      model: _model,
      prices: _prices,
      screenplayLocked: _screenplayLocked,
      bibleCostMicros: _bibleCost,
      working: _working,
      workingLabel: _workingLabel,
      error: _error,
      onResubmit: _resubmit,
      editingEntry: _editingEntry,
      navigation: widget.navigation,
      isDarkMode: widget.isDarkMode,
      onThemeToggle: widget.onThemeToggle,
      onWriteBible: () => _run(
        project.writeBible,
        'Writing the bible with ${_model.kind.label}',
        resubmit: () => project.writeBible(resubmit: true),
      ),
      onEditEntry: (key) => setState(() {
        _editingEntry = key;
        widget.work.dismiss();
      }),
      onSaveEntry: (edit) => _run(
        () => project.saveBibleEntry(_production.bible!.revision, edit),
        'Saving',
        onDone: () => _editingEntry = null,
      ),
      onLockBible: (revision) => _run(() => project.lockBible(revision), 'Locking'),
      onSample: (preset, note) => _run(
        () => project.sampleStyle(preset, note: note),
        'Sampling the style',
        resubmit: () => project.sampleStyle(preset, note: note, resubmit: true),
      ),
      onRefine: (sample, note) => _run(
        () => project.refineStyle(sample, note),
        'Refining the style',
        resubmit: () => project.refineStyle(sample, note, resubmit: true),
      ),
      onSelectStyle: (sample) => _run(() => project.selectStyle(sample), 'Selecting'),
      onRejectStyle: (sample) => _run(() => project.rejectStyle(sample), 'Rejecting'),
      onLockStyle: (sample) => _run(() => project.lockStyle(sample), 'Locking'),
      onAudition: (character, count) => _run(
        () => project.audition(character, count),
        'Casting',
        resubmit: () => project.audition(character, count, resubmit: true),
      ),
      onCastPortrait: (character, portrait) =>
          _run(() => project.castPortrait(character, portrait), 'Casting'),
      onRejectPortrait: (character, portrait) =>
          _run(() => project.rejectPortrait(character, portrait), 'Rejecting'),
    );
  }
}

class ProductionWorkspaceView extends StatelessWidget {
  const ProductionWorkspaceView({
    required this.production,
    required this.model,
    required this.onWriteBible,
    required this.onEditEntry,
    required this.onSaveEntry,
    required this.onLockBible,
    required this.onSample,
    required this.onRefine,
    required this.onSelectStyle,
    required this.onRejectStyle,
    required this.onLockStyle,
    required this.onAudition,
    required this.onCastPortrait,
    required this.onRejectPortrait,
    this.references = const <KinetoReference>[],
    this.servers,
    this.serversProblem,
    this.loadingTools = false,
    this.onLoadTools,
    this.onAskReference,
    this.onEditReference,
    this.onAttachReference,
    this.onRemoveReference,
    this.onImportPortrait,
    this.onImportStyle,
    this.prices = const ProductionPrices(),
    this.screenplayLocked = false,
    this.bibleCostMicros,
    this.working = false,
    this.workingLabel = 'Working',
    this.error,
    this.onResubmit,
    this.editingEntry,
    this.navigation,
    this.isDarkMode = true,
    this.onThemeToggle,
    super.key,
  });

  final KinetoProduction production;
  final KinetoLanguageModel model;
  final ProductionPrices prices;

  /// The bible is written from the locked screenplay.
  final bool screenplayLocked;
  final int? bibleCostMicros;

  /// A command is running on the project's worker; changes wait for it.
  final bool working;
  final String workingLabel;
  final String? error;

  /// Asks again at the risk of paying twice; set only when [error] says the
  /// last paid request may have been charged without its result.
  final VoidCallback? onResubmit;

  /// The bible entry being edited, as `<kind>-<id>`, if any.
  final String? editingEntry;
  final Widget? navigation;
  final bool isDarkMode;
  final VoidCallback? onThemeToggle;

  final VoidCallback onWriteBible;

  /// Start editing an entry, or stop with null.
  final ValueChanged<String?> onEditEntry;
  final ValueChanged<KinetoBibleEntryEdit> onSaveEntry;
  final ValueChanged<int> onLockBible;
  final void Function(String preset, String? note) onSample;
  final void Function(String sample, String note) onRefine;
  final ValueChanged<String> onSelectStyle;
  final ValueChanged<String> onRejectStyle;
  final ValueChanged<String> onLockStyle;
  final void Function(String character, int count) onAudition;
  /// Select a portrait and lock the character as it, in one step.
  final void Function(String character, String portrait) onCastPortrait;
  final void Function(String character, String portrait) onRejectPortrait;

  /// References from MCP tools, oldest first.
  final List<KinetoReference> references;

  /// The registered MCP servers and their tools; null until loaded.
  final List<KinetoMcpServer>? servers;
  final String? serversProblem;
  final bool loadingTools;
  final VoidCallback? onLoadTools;
  final void Function(String server, String tool, List<KinetoToolArgument> arguments, List<String> targets)?
      onAskReference;
  final void Function(String reference, String text)? onEditReference;
  final void Function(String reference, String target, bool attach)? onAttachReference;
  final ValueChanged<String>? onRemoveReference;

  /// Import an image the director chooses as a portrait of the character.
  final ValueChanged<String>? onImportPortrait;

  /// Import an image the director chooses, with their description of its
  /// look, as a style sample.
  final VoidCallback? onImportStyle;

  bool get _idle => !working && editingEntry == null;

  @override
  Widget build(BuildContext context) => WabScaffold(
        appBar: WabAppBar(
          title: const Text('Kineto'),
          action: onThemeToggle == null
              ? null
              : KinetoThemeToggle(isDarkMode: isDarkMode, onToggle: onThemeToggle),
        ),
        body: SingleChildScrollView(
          padding: const EdgeInsets.all(24),
          child: Center(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 1040),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: <Widget>[
                  if (navigation != null) ...<Widget>[navigation!, const SizedBox(height: 20)],
                  if (error != null) ...<Widget>[_errorPanel(), const SizedBox(height: 20)],
                  if (production.damaged) ...<Widget>[
                    const WabPanel(
                      title: 'The bible, style or cast cannot be read',
                      child: Text(
                        "A stored record no longer matches its files. Nothing was changed; restore the "
                        "project's files from a backup or version control.",
                      ),
                    ),
                    const SizedBox(height: 20),
                  ],
                  _biblePanel(context),
                  const SizedBox(height: 20),
                  if (onAskReference != null) ...<Widget>[
                    ReferencesPanel(
                      references: references,
                      targets: referenceTargets(production.bible),
                      enabled: _idle,
                      servers: servers,
                      serversProblem: serversProblem,
                      loadingTools: loadingTools,
                      onLoadTools: onLoadTools ?? () {},
                      onAsk: onAskReference!,
                      onEdit: onEditReference ?? (_, _) {},
                      onAttach: onAttachReference ?? (_, _, _) {},
                      onRemove: onRemoveReference ?? (_) {},
                    ),
                    const SizedBox(height: 20),
                  ],
                  _stylePanel(context),
                  const SizedBox(height: 20),
                  _castPanel(context),
                ],
              ),
            ),
          ),
        ),
      );

  Widget _errorPanel() => WabPanel(
        title: 'That did not work',
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: <Widget>[
            Text(error!),
            if (onResubmit != null) ...<Widget>[
              const SizedBox(height: 12),
              const Text(
                'The provider may have answered the last request, and charged for it, without the '
                'answer reaching Kineto. Check your account before asking again.',
              ),
              const SizedBox(height: 12),
              KinetoButton(
                key: const ValueKey<String>('resubmit-paid'),
                onPressed: working ? null : onResubmit,
                child: const Text('Ask again · may pay twice'),
              ),
            ],
          ],
        ),
      );

  // ---------------------------------------------------------------- bible

  Widget _biblePanel(BuildContext context) {
    final bible = production.bible;
    final problem = languageModelProblem(model);
    final (status, kind) = bible == null
        ? ('None yet', WabBadgeKind.neutral)
        : production.bibleIsLocked
            ? (production.bibleStale.isEmpty ? 'Locked' : 'Locked · screenplay changed since', WabBadgeKind.done)
            : production.bibleStale.isNotEmpty
                ? ('Screenplay changed since', WabBadgeKind.warning)
                : ('Draft', WabBadgeKind.progress);
    return WabPanel(
      title: bible == null ? 'Bible' : 'Bible · revision ${bible.revision}',
      trailing: WabStatusBadge(status, kind: kind),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          if (bible == null)
            Text(
              screenplayLocked
                  ? 'Not written yet. The bible describes the world, every character, location and '
                      'prop so that every image shows the same ones.'
                  : 'Lock a screenplay first; the bible is written from it.',
            )
          else ...<Widget>[
            if (production.bibleStale.isNotEmpty) ...<Widget>[
              Text('May no longer match the locked screenplay or its references: '
                  '${production.bibleStale.map((cause) => cause.description).join('; ')}.'),
              const SizedBox(height: 12),
            ],
            Text(bible.setting, style: Theme.of(context).textTheme.titleMedium),
            const SizedBox(height: 4),
            Text(bible.period),
            for (final rule in <String>[...bible.visualRules, ...bible.continuityRules])
              Text('· $rule'),
            const SizedBox(height: 16),
            for (final entry in <KinetoBibleEntry>[...bible.characters, ...bible.locations, ...bible.props])
              _entryKey(entry) == editingEntry
                  ? BibleEntryEditor(
                      key: ValueKey<String>('entry-editor-${bible.revision}-${_entryKey(entry)}'),
                      entry: entry,
                      working: working,
                      onCancel: () => onEditEntry(null),
                      onSave: onSaveEntry,
                    )
                  : _entry(context, entry),
          ],
          const SizedBox(height: 12),
          if (problem != null) ...<Widget>[Text(problem), const SizedBox(height: 12)],
          Wrap(
            spacing: 12,
            runSpacing: 12,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: <Widget>[
              KinetoButton(
                key: const ValueKey<String>('write-bible'),
                primary: true,
                onPressed: _idle && model.ready && screenplayLocked ? onWriteBible : null,
                child: Text(bible == null ? 'Write the bible' : 'Write it again'),
              ),
              if (bible != null && !production.bibleIsLocked)
                KinetoButton(
                  key: const ValueKey<String>('lock-bible'),
                  primary: true,
                  onPressed: _idle ? () => onLockBible(bible.revision) : null,
                  child: Text('Lock revision ${bible.revision}'),
                ),
              Text('${model.kind.label} · ${documentCostLabel(model, bibleCostMicros)}'),
            ],
          ),
          if (production.bibleRevisions.isNotEmpty) ...<Widget>[
            const SizedBox(height: 12),
            for (final revision in production.bibleRevisions.reversed) Text(describeRevision(revision)),
          ],
        ],
      ),
    );
  }

  static String _entryKey(KinetoBibleEntry entry) => '${entry.kind.wireName}-${entry.id}';

  Widget _entry(BuildContext context, KinetoBibleEntry entry) => Padding(
        padding: const EdgeInsets.only(bottom: 16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: <Widget>[
            Row(
              children: <Widget>[
                Expanded(
                  child: Text(
                    '${entry.name} · ${entry.kind.wireName}',
                    style: Theme.of(context).textTheme.titleSmall,
                  ),
                ),
                KinetoButton(
                  key: ValueKey<String>('edit-${_entryKey(entry)}'),
                  onPressed: _idle ? () => onEditEntry(_entryKey(entry)) : null,
                  child: const Text('Edit'),
                ),
              ],
            ),
            Text(entry.description),
            Text(entry.appearance),
            if (entry.wardrobe.isNotEmpty) Text('Wardrobe: ${entry.wardrobe}'),
            for (final change in entry.wardrobeChanges) Text('In ${change.scenes}: ${change.wardrobe}'),
            if (entry.forbiddenChanges.isNotEmpty)
              Text('Never changes: ${entry.forbiddenChanges.join(' ')}'),
          ],
        ),
      );

  // ---------------------------------------------------------------- style

  Widget _stylePanel(BuildContext context) {
    final style = production.style;
    return WabPanel(
      title: 'Style',
      trailing: style == null
          ? const WabStatusBadge('Not locked', kind: WabBadgeKind.neutral)
          : WabStatusBadge('Locked · ${style.preset}', kind: WabBadgeKind.done),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          if (production.styleUnresolved) ...<Widget>[
            const Text(
              'A style sample did not finish cleanly, and the image provider may already have charged '
              'for an image Kineto never received. Check your account with it; sample again only if you '
              'accept paying twice.',
            ),
            const SizedBox(height: 12),
          ],
          StyleComposer(
            presets: production.presets,
            cost: prices.sample,
            enabled: _idle && prices.makesImages,
            onSample: onSample,
          ),
          if (!prices.makesImages) ...<Widget>[
            const SizedBox(height: 8),
            const Text(
              'Style samples need an image provider that makes images: choose one under Settings, '
              'Providers, and save its key if it takes one.',
            ),
          ],
          const SizedBox(height: 12),
          Wrap(
            spacing: 12,
            runSpacing: 8,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: <Widget>[
              KinetoButton(
                key: const ValueKey<String>('import-style'),
                onPressed: _idle ? onImportStyle : null,
                child: const Text('Import an image as a style'),
              ),
              const Text(
                'Free. Prompts follow your description of it; a provider that takes a style reference is '
                'sent the image too.',
              ),
            ],
          ),
          if (production.samples.isNotEmpty) ...<Widget>[
            const SizedBox(height: 16),
            Wrap(
              spacing: 16,
              runSpacing: 16,
              children: <Widget>[
                for (final sample in production.samples.reversed) _sample(context, sample),
              ],
            ),
          ],
        ],
      ),
    );
  }

  Widget _sample(BuildContext context, KinetoStyleSample sample) {
    final locked = production.style?.sampleId == sample.id;
    final rejected = sample.status == KinetoChoiceStatus.rejected;
    final selected = sample.status == KinetoChoiceStatus.selected;
    return SizedBox(
      width: 300,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          Opacity(
            opacity: rejected ? .45 : 1,
            child: MediaFrame(
              path: sample.mediaPath,
              aspectRatio: 16 / 9,
              gallery: <String>[for (final sample in production.samples.reversed) sample.mediaPath],
            ),
          ),
          const SizedBox(height: 8),
          Text(
            '${sample.id} · ${sample.imported ? 'imported' : sample.preset}'
            '${locked ? ' · locked' : selected ? ' · selected' : rejected ? ' · rejected' : ''}',
            style: Theme.of(context).textTheme.titleSmall,
          ),
          if (sample.imported) Text(sample.directives, key: ValueKey<String>('style-description-${sample.id}')),
          if (sample.note != null) Text(sample.note!),
          if (sample.refines != null) Text('Refines ${sample.refines}'),
          const SizedBox(height: 8),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: <Widget>[
              if (!selected && !rejected)
                KinetoButton(
                  key: ValueKey<String>('select-${sample.id}'),
                  onPressed: _idle ? () => onSelectStyle(sample.id) : null,
                  child: const Text('Select'),
                ),
              if (selected && !locked)
                KinetoButton(
                  key: ValueKey<String>('lock-${sample.id}'),
                  primary: true,
                  onPressed: _idle ? () => onLockStyle(sample.id) : null,
                  child: const Text('Lock style'),
                ),
              if (!rejected && !locked)
                KinetoButton(
                  key: ValueKey<String>('reject-${sample.id}'),
                  onPressed: _idle ? () => onRejectStyle(sample.id) : null,
                  child: const Text('Reject'),
                ),
              // An imported image has no preset to sample again from.
              if (!sample.imported)
                KinetoButton(
                  key: ValueKey<String>('refine-${sample.id}'),
                  onPressed: _idle && prices.makesImages ? () => _refineDialog(context, sample) : null,
                  child: const Text('Refine'),
                ),
            ],
          ),
        ],
      ),
    );
  }

  Future<void> _refineDialog(BuildContext context, KinetoStyleSample sample) async {
    final note = await showDialog<String>(
      context: context,
      builder: (context) => TextPromptDialog(
        title: 'Refine ${sample.id}',
        width: 520,
        fieldKey: 'refine-note',
        minLines: 2,
        maxLines: 4,
        hint: 'What should change, such as "warmer light, less grain"',
        confirmKey: 'refine-confirm',
        confirmLabel: 'Sample · ${imageCostLabel(prices.sample)}',
      ),
    );
    if (note == null || note.trim().isEmpty) return;
    onRefine(sample.id, note.trim());
  }

  // ----------------------------------------------------------------- cast

  Widget _castPanel(BuildContext context) => WabPanel(
        title: 'Cast',
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: <Widget>[
            if (production.characters.isEmpty)
              const Text('Write the bible first; the characters worth casting come from it.')
            else if (production.style == null)
              const Text('Lock a style first; every portrait follows it.'),
            for (final character in production.characters) ...<Widget>[
              const SizedBox(height: 12),
              CastingCard(
                character: character,
                prices: prices,
                enabled: _idle && production.style != null,
                onAudition: (count) => onAudition(character.id, count),
                onCast: (portrait) => onCastPortrait(character.id, portrait),
                onRejectPortrait: (portrait) => onRejectPortrait(character.id, portrait),
                onImport: onImportPortrait == null ? null : () => onImportPortrait!(character.id),
              ),
            ],
          ],
        ),
      );
}

/// Chooses a style preset and asks for a sample of it.
class StyleComposer extends StatefulWidget {
  const StyleComposer({
    required this.presets,
    required this.cost,
    required this.enabled,
    required this.onSample,
    super.key,
  });

  final List<KinetoStylePreset> presets;
  final KinetoImageCost? cost;
  final bool enabled;
  final void Function(String preset, String? note) onSample;

  @override
  State<StyleComposer> createState() => _StyleComposerState();
}

class _StyleComposerState extends State<StyleComposer> {
  final TextEditingController _note = TextEditingController();
  String? _preset;

  @override
  void dispose() {
    _note.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final preset = _preset ?? (widget.presets.isEmpty ? null : widget.presets.first.id);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: <Widget>[
        Wrap(
          spacing: 8,
          runSpacing: 8,
          children: <Widget>[
            for (final option in widget.presets)
              KinetoButton(
                key: ValueKey<String>('preset-${option.id}'),
                primary: option.id == preset,
                onPressed: () => setState(() => _preset = option.id),
                child: Text(option.name),
              ),
          ],
        ),
        const SizedBox(height: 12),
        TextField(
          key: const ValueKey<String>('style-note'),
          controller: _note,
          style: TextStyle(color: WabTheme.textColor),
          decoration: wabInputDecoration(hintText: 'A note for this sample (optional)'),
        ),
        const SizedBox(height: 12),
        Align(
          alignment: Alignment.centerLeft,
          child: KinetoButton(
            key: const ValueKey<String>('sample-style'),
            primary: true,
            onPressed: widget.enabled && preset != null
                ? () {
                    final note = _note.text.trim();
                    widget.onSample(preset, note.isEmpty ? null : note);
                  }
                : null,
            child: Text('Sample · ${imageCostLabel(widget.cost)}'),
          ),
        ),
      ],
    );
  }
}

/// One character's casting: portrait candidates and the one chosen, which
/// casts the character.
class CastingCard extends StatefulWidget {
  const CastingCard({
    required this.character,
    required this.prices,
    required this.enabled,
    required this.onAudition,
    required this.onCast,
    required this.onRejectPortrait,
    this.onImport,
    super.key,
  });

  final KinetoCastCharacter character;
  final ProductionPrices prices;
  final bool enabled;
  final ValueChanged<int> onAudition;

  /// Cast the character as this portrait: selected and locked at once.
  final ValueChanged<String> onCast;
  final ValueChanged<String> onRejectPortrait;

  /// Import an image the director chooses as a portrait; free.
  final VoidCallback? onImport;

  @override
  State<CastingCard> createState() => _CastingCardState();
}

class _CastingCardState extends State<CastingCard> {
  int _have = 3;

  @override
  Widget build(BuildContext context) {
    final character = widget.character;
    final id = character.id;
    final enabled = widget.enabled;
    final kept = character.portraits.where((image) => image.status != KinetoChoiceStatus.rejected).length;
    final toMake = _have > kept ? _have - kept : 0;
    final (status, kind) = character.locked
        ? character.stale.isEmpty
            ? ('Cast', WabBadgeKind.done)
            : ('Cast · changed since: ${character.stale.join(', ')}', WabBadgeKind.warning)
        : ('Not cast', WabBadgeKind.neutral);
    return WabPanel(
      title: character.name,
      trailing: WabStatusBadge(status, kind: kind),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          if (character.unresolved) ...<Widget>[
            Text(
              'An image of ${character.name} did not finish cleanly, and the image provider may already '
              'have charged for it. Check your account with it; ask again only if you accept paying twice.',
            ),
            const SizedBox(height: 12),
          ],
          Wrap(
            spacing: 12,
            runSpacing: 12,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: <Widget>[
              const Text('Have'),
              SizedBox(
                width: 96,
                child: WabDropdown<int>(
                  key: ValueKey<String>('audition-count-$id'),
                  value: _have,
                  items: <int, String>{for (final size in auditionSizes) size: '$size'},
                  onChanged: (size) => setState(() => _have = size ?? _have),
                ),
              ),
              KinetoButton(
                key: ValueKey<String>('audition-$id'),
                primary: true,
                onPressed: enabled && toMake > 0 && widget.prices.makesImages
                    ? () => widget.onAudition(_have)
                    : null,
                child: Text(
                  toMake == 0
                      ? 'Portraits ready'
                      : 'Audition · ${imageCostLabel(timesImages(widget.prices.portrait, toMake))}',
                ),
              ),
              if (widget.onImport != null)
                KinetoButton(
                  key: ValueKey<String>('import-portrait-$id'),
                  onPressed: enabled ? widget.onImport : null,
                  child: const Text('Import a portrait · free'),
                ),
            ],
          ),
          if (character.portraits.isNotEmpty) ...<Widget>[
            const SizedBox(height: 12),
            if (!character.locked) const Text('Choose the portrait that plays the part; it is cast at once.'),
            const SizedBox(height: 8),
            Wrap(
              spacing: 12,
              runSpacing: 12,
              children: <Widget>[
                for (final portrait in character.portraits) _portrait(portrait),
              ],
            ),
          ],
        ],
      ),
    );
  }

  Widget _portrait(KinetoCastImage portrait) {
    final rejected = portrait.status == KinetoChoiceStatus.rejected;
    final locked = widget.character.lockedPortrait == portrait.id;
    return SizedBox(
      width: 180,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          Opacity(
            opacity: rejected ? .45 : 1,
            child: MediaFrame(
              path: portrait.mediaPath,
              aspectRatio: 1,
              gallery: <String>[for (final portrait in widget.character.portraits) portrait.mediaPath],
            ),
          ),
          const SizedBox(height: 6),
          Text('${portrait.id}${locked ? ' · cast' : rejected ? ' · rejected' : ''}'),
          const SizedBox(height: 6),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: <Widget>[
              if (!locked && !rejected)
                KinetoButton(
                  key: ValueKey<String>('cast-${portrait.id}'),
                  primary: true,
                  onPressed: widget.enabled ? () => widget.onCast(portrait.id) : null,
                  child: const Text('Cast'),
                ),
              if (!rejected && !locked)
                KinetoButton(
                  key: ValueKey<String>('reject-${portrait.id}'),
                  onPressed: widget.enabled ? () => widget.onRejectPortrait(portrait.id) : null,
                  child: const Text('Reject'),
                ),
            ],
          ),
        ],
      ),
    );
  }
}

/// Edits one bible entry: what it looks like and what must never change.
class BibleEntryEditor extends StatefulWidget {
  const BibleEntryEditor({
    required this.entry,
    required this.onCancel,
    required this.onSave,
    this.working = false,
    super.key,
  });

  final KinetoBibleEntry entry;
  final bool working;
  final VoidCallback onCancel;
  final ValueChanged<KinetoBibleEntryEdit> onSave;

  @override
  State<BibleEntryEditor> createState() => _BibleEntryEditorState();
}

class _BibleEntryEditorState extends State<BibleEntryEditor> {
  late final TextEditingController _description = TextEditingController(text: widget.entry.description);
  late final TextEditingController _appearance = TextEditingController(text: widget.entry.appearance);
  late final TextEditingController _wardrobe = TextEditingController(text: widget.entry.wardrobe);
  late final TextEditingController _forbidden =
      TextEditingController(text: widget.entry.forbiddenChanges.join('\n'));
  String? _problem;

  bool get _character => widget.entry.kind == KinetoBibleEntryKind.character;

  @override
  void dispose() {
    _description.dispose();
    _appearance.dispose();
    _wardrobe.dispose();
    _forbidden.dispose();
    super.dispose();
  }

  void _save() {
    final description = _description.text.trim();
    final appearance = _appearance.text.trim();
    final wardrobe = _wardrobe.text.trim();
    if (description.isEmpty || appearance.isEmpty || (_character && wardrobe.isEmpty)) {
      setState(() => _problem = _character
          ? 'A character needs a description, an appearance and a wardrobe.'
          : 'An entry needs a description and an appearance.');
      return;
    }
    setState(() => _problem = null);
    widget.onSave(
      KinetoBibleEntryEdit(
        kind: widget.entry.kind,
        id: widget.entry.id,
        description: description,
        appearance: appearance,
        wardrobe: wardrobe,
        forbiddenChanges: <String>[
          for (final line in _forbidden.text.split('\n'))
            if (line.trim().isNotEmpty) line.trim(),
        ],
      ),
    );
  }

  Widget _field(String key, String label, TextEditingController controller, {int lines = 2}) => Padding(
        padding: const EdgeInsets.only(bottom: 12),
        child: TextField(
          key: ValueKey<String>(key),
          controller: controller,
          minLines: lines,
          maxLines: lines + 4,
          style: TextStyle(color: WabTheme.textColor),
          decoration: wabInputDecoration(hintText: label),
        ),
      );

  @override
  Widget build(BuildContext context) {
    final id = widget.entry.id;
    return Padding(
      padding: const EdgeInsets.only(bottom: 16),
      child: WabPanel(
        title: 'Editing ${widget.entry.name}',
        trailing: const WabStatusBadge('Editing', kind: WabBadgeKind.warning),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: <Widget>[
            _field('entry-$id-description', 'Description', _description),
            _field('entry-$id-appearance', 'Appearance', _appearance, lines: 3),
            if (_character) _field('entry-$id-wardrobe', 'Usual wardrobe', _wardrobe),
            _field('entry-$id-forbidden', 'Never changes, one per line', _forbidden, lines: 3),
            if (_problem != null) ...<Widget>[Text(_problem!), const SizedBox(height: 12)],
            Wrap(
              spacing: 12,
              children: <Widget>[
                KinetoButton(
                  key: ValueKey<String>('save-entry-$id'),
                  primary: true,
                  onPressed: widget.working ? null : _save,
                  child: const Text('Save as a new revision'),
                ),
                KinetoButton(
                  onPressed: widget.working ? null : widget.onCancel,
                  child: const Text('Cancel'),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}
