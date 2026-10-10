import 'package:flutter/material.dart';
import 'package:wabisabi/wabisabi.dart';

import 'kineto_controls.dart';
import 'kineto_layout.dart';
import 'kineto_work.dart';
import 'media_player.dart';
import 'native/kineto_engine.dart';
import 'screenplay_workspace.dart';

/// What speech costs, in words: `free`, `≈ $0.0028`, or `cost unknown`.
String speechCostLabel(int? micros) {
  if (micros == null) return 'cost unknown';
  if (micros == 0) return 'free';
  return '≈ ${formatDollars(micros)}';
}

/// [millis] as seconds for a person, for example `1.4 s`.
String formatSeconds(int millis) => '${(millis / 1000).toStringAsFixed(1)} s';

/// How many takes one dubbing request may ask for.
const List<int> takeCounts = <int>[1, 2, 3, 4];

/// What saying a text a number of times costs; null when unknown.
typedef SpeechCost = int? Function(String text, int count);

class VoicesWorkspaceScreen extends StatefulWidget {
  const VoicesWorkspaceScreen({
    required this.work,
    required this.project,
    super.key,
  });

  final KinetoProjectSession project;


  /// The project's requests, which outlive this page.
  final KinetoWork work;

  @override
  State<VoicesWorkspaceScreen> createState() => _VoicesWorkspaceScreenState();
}

class _VoicesWorkspaceScreenState extends State<VoicesWorkspaceScreen> {
  late final KinetoLanguageModel _model;
  late KinetoVoices _voices;
  int? _recommendCost;
  static const String _page = 'voices';
  int _finished = 0;

  // The project's requests outlive this page: what is under way, and the
  // newest one asked for here that failed, come from them; a request that
  // succeeds for one thing leaves another's failure shown.
  // Each character's voice and each line is changed one request at a time;
  // different ones at once.
  String? get _error => widget.work.problemOn(_page);
  VoidCallback? get _resubmit => widget.work.retryOn(_page);


  @override
  void initState() {
    super.initState();
    _finished = widget.work.finished;
    widget.work.addListener(_workChanged);
    _model = widget.project.languageModel;
    _read();
  }

  void _read() {
    _voices = widget.project.voices;
    _recommendCost = widget.project.documentCostMicros(KinetoDocumentKind.voiceRecommendations);
  }

  // Speech is made on the project's worker; this isolate only waits. One
  // command at a time keeps the page honest about what it shows.
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
    required String target,
    Future<void> Function()? resubmit,
  }) async {
    await widget.work.run(_page, label, action, target: target, retry: resubmit);
  }

  @override
  Widget build(BuildContext context) {
    final project = widget.project;
    return VoicesWorkspaceView(
      voices: _voices,
      model: _model,
      recommendCostMicros: _recommendCost,
      speechCost: (text, count) => project.speechCostMicros(text, count: count),
      busy: widget.work.targets,
      error: _error,
      onResubmit: _resubmit,
      onLoadVoices: () => _run(project.loadVoices, 'Listing voices', target: 'catalog'),
      onRecommend: () => _run(
        project.recommendVoices,
        'Asking ${_model.writer} for voices',
        target: 'recommendations',
        resubmit: () => project.recommendVoices(resubmit: true),
      ),
      onAudition: (character, choices, line) => _run(
        () => project.auditionVoices(character, choices, line: line),
        'Auditioning',
        target: 'voice:$character',
        resubmit: () => project.auditionVoices(character, choices, line: line, resubmit: true),
      ),
      onSelectVoice: (character, audition) =>
          _run(() => project.selectVoice(character, audition), 'Selecting', target: 'voice:$character'),
      onRejectVoice: (character, audition) =>
          _run(() => project.rejectVoice(character, audition), 'Rejecting', target: 'voice:$character'),
      onLockVoice: (character) => _run(() => project.lockVoice(character), 'Locking', target: 'voice:$character'),
      onDub: (line, request) => _run(
        () => project.dubLine(
          line.sceneId,
          line.number,
          takes: request.takes,
          delivery: request.delivery,
          note: request.note,
        ),
        'Dubbing',
        target: lineTarget(line),
        resubmit: () => project.dubLine(
          line.sceneId,
          line.number,
          takes: request.takes,
          delivery: request.delivery,
          note: request.note,
          resubmit: true,
        ),
      ),
      onSelectTake: (line, take) =>
          _run(() => project.selectTake(line.sceneId, take), 'Selecting', target: lineTarget(line)),
      onRejectTake: (line, take) =>
          _run(() => project.rejectTake(line.sceneId, take), 'Rejecting', target: lineTarget(line)),
      onLockLine: (line) =>
          _run(() => project.lockLine(line.sceneId, line.number), 'Locking', target: lineTarget(line)),
    );
  }
}

/// What requests about [line] work on.
String lineTarget(KinetoLine line) => 'line:${line.sceneId}:${line.number}';

/// What the director asks of a line's takes.
final class DubRequest {
  const DubRequest({this.takes = 1, this.delivery, this.note});

  final int takes;

  /// Instead of the screenplay's delivery; null keeps it.
  final KinetoEmotion? delivery;
  final String? note;
}

class VoicesWorkspaceView extends StatelessWidget {
  const VoicesWorkspaceView({
    required this.voices,
    required this.model,
    required this.speechCost,
    required this.onLoadVoices,
    required this.onRecommend,
    required this.onAudition,
    required this.onSelectVoice,
    required this.onRejectVoice,
    required this.onLockVoice,
    required this.onDub,
    required this.onSelectTake,
    required this.onRejectTake,
    required this.onLockLine,
    this.recommendCostMicros,
    this.busy = const <String>{},
    this.workingLabel = 'Working',
    this.error,
    this.onResubmit,
    super.key,
  });

  final KinetoVoices voices;
  final KinetoLanguageModel model;
  final SpeechCost speechCost;
  final int? recommendCostMicros;

  /// A command is running on the project's worker; changes wait for it.
  /// What requests under way work on, such as `voice:mira` or
  /// `line:office:3`: the controls for those wait.
  final Set<String> busy;
  final String workingLabel;
  final String? error;

  /// Asks again at the risk of paying twice; set only when [error] says the
  /// last paid request may have been charged without its result.
  final VoidCallback? onResubmit;

  /// Opens an audio file with the system's player.
  final VoidCallback onLoadVoices;
  final VoidCallback onRecommend;
  final void Function(String character, List<KinetoVoiceChoice> choices, String? line) onAudition;
  final void Function(String character, String audition) onSelectVoice;
  final void Function(String character, String audition) onRejectVoice;
  final ValueChanged<String> onLockVoice;
  final void Function(KinetoLine line, DubRequest request) onDub;
  final void Function(KinetoLine line, String take) onSelectTake;
  final void Function(KinetoLine line, String take) onRejectTake;
  final ValueChanged<KinetoLine> onLockLine;

  bool _free(String target) => !busy.contains(target);

  String _speakerName(String? characterId) {
    if (characterId == null) return 'No one';
    return voices.character(characterId)?.characterName ?? characterId;
  }

  @override
  Widget build(BuildContext context) => KinetoWorkspace(
        title: 'Voices & dialogue',
        sideWidth: 520,
        notices: <Widget>[if (error != null) _errorPanel()],
        // Who speaks, and with which voice.
        side: <Widget>[_providerPanel(), _castPanel()],
        // What they say, line by line.
        main: <Widget>[_linesPanel()],
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
                onPressed: onResubmit,
                child: const Text('Ask again'),
              ),
            ],
          ],
        ),
      );

  Widget _providerPanel() {
    final perThousand = voices.characterCostMicros == null ? null : voices.characterCostMicros! * 1000;
    return WabPanel(
      title: 'Speech',
      trailing: WabStatusBadge(
        voices.speechReady ? voices.speechProvider.label : '${voices.speechProvider.label} · no key',
        kind: voices.speechReady ? WabBadgeKind.neutral : WabBadgeKind.warning,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Text(
            perThousand == null
                ? 'The provider does not say what speech costs.'
                : perThousand == 0
                    ? 'Speech is free with this provider.'
                    : '${formatDollars(perThousand)} per 1,000 characters spoken.',
          ),
          const SizedBox(height: 8),
          Text(
            voices.voices.isEmpty
                ? 'List the provider\'s voices to audition them.'
                : '${voices.voices.length} voices listed.',
          ),
          if (voices.voicesProblem != null) ...<Widget>[
            const SizedBox(height: 8),
            Text('The voices could not be listed: ${voices.voicesProblem}'),
          ],
          const SizedBox(height: 12),
          Wrap(
            spacing: 12,
            runSpacing: 8,
            children: <Widget>[
              KinetoButton(
                key: const ValueKey<String>('load-voices'),
                onPressed: _free('catalog') && voices.speechReady ? onLoadVoices : null,
                child: Text(voices.voices.isEmpty ? 'List voices · free' : 'List again · free'),
              ),
              KinetoButton(
                key: const ValueKey<String>('recommend-voices'),
                onPressed: _free('recommendations') && model.ready && voices.characters.isNotEmpty ? onRecommend : null,
                child: Text('Recommend voices · ${documentCostLabel(model, recommendCostMicros)}'),
              ),
            ],
          ),
        ],
      ),
    );
  }

  Widget _castPanel() => WabPanel(
        title: 'Voices',
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: <Widget>[
            if (voices.characters.isEmpty)
              const Text('Write the bible first; the characters worth voicing come from it.'),
            for (final character in voices.characters) ...<Widget>[
              const SizedBox(height: 12),
              VoiceCastingCard(
                character: character,
                catalog: voices.voices,
                speechCost: speechCost,
                enabled: _free('voice:${character.characterId}') && voices.speechReady,
                onAudition: (choices, line) => onAudition(character.characterId, choices, line),
                onSelect: (audition) => onSelectVoice(character.characterId, audition),
                onReject: (audition) => onRejectVoice(character.characterId, audition),
                onLock: () => onLockVoice(character.characterId),
              ),
            ],
          ],
        ),
      );

  Widget _linesPanel() {
    // Lines come in screenplay order; each scene once.
    final scenes = <String>[];
    for (final line in voices.lines) {
      if (scenes.isEmpty || scenes.last != line.sceneId) scenes.add(line.sceneId);
    }
    return WabPanel(
      title: 'Dialogue',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          if (voices.lines.isEmpty) const Text('The screenplay has no lines to dub.'),
          for (final scene in scenes) ...<Widget>[
            const SizedBox(height: 12),
            Text(scene.replaceAll('_', ' '), style: const TextStyle(fontWeight: FontWeight.w600)),
            for (final line in voices.linesOf(scene)) ...<Widget>[
              const SizedBox(height: 8),
              LineDubbingCard(
                line: line,
                speaker: _speakerName(line.characterId),
                voiceLocked: line.characterId != null && (voices.character(line.characterId!)?.locked ?? false),
                speechCost: speechCost,
                enabled: _free(lineTarget(line)) && voices.speechReady,
                onDub: (request) => onDub(line, request),
                onSelect: (take) => onSelectTake(line, take),
                onReject: (take) => onRejectTake(line, take),
                onLock: () => onLockLine(line),
              ),
            ],
          ],
        ],
      ),
    );
  }
}

/// One character's voice: auditions to hear, one to choose and lock.
class VoiceCastingCard extends StatefulWidget {
  const VoiceCastingCard({
    required this.character,
    required this.catalog,
    required this.speechCost,
    required this.enabled,
    required this.onAudition,
    required this.onSelect,
    required this.onReject,
    required this.onLock,
    super.key,
  });

  final KinetoCharacterVoice character;
  final List<KinetoVoice> catalog;
  final SpeechCost speechCost;
  final bool enabled;
  final void Function(List<KinetoVoiceChoice> choices, String? line) onAudition;
  final ValueChanged<String> onSelect;
  final ValueChanged<String> onReject;
  final VoidCallback onLock;

  @override
  State<VoiceCastingCard> createState() => _VoiceCastingCardState();
}

class _VoiceCastingCardState extends State<VoiceCastingCard> {
  final Set<String> _chosen = <String>{};
  final TextEditingController _line = TextEditingController();
  KinetoEmotion? _delivery;

  @override
  void dispose() {
    _line.dispose();
    super.dispose();
  }

  String get _said => _line.text.trim().isEmpty ? widget.character.defaultLine : _line.text.trim();

  /// The catalog, the model's recommendations first.
  List<KinetoVoice> get _ordered {
    final recommended = <String>{for (final voice in widget.character.recommended) voice.voiceId};
    return <KinetoVoice>[
      ...widget.catalog.where((voice) => recommended.contains(voice.id)),
      ...widget.catalog.where((voice) => !recommended.contains(voice.id)),
    ];
  }

  String? _reason(String voiceId) {
    for (final voice in widget.character.recommended) {
      if (voice.voiceId == voiceId) return voice.reason;
    }
    return null;
  }

  @override
  Widget build(BuildContext context) {
    final character = widget.character;
    final id = character.characterId;
    final cost = speechCostLabel(widget.speechCost(_said, _chosen.length));
    final selected = character.auditions.where((audition) => audition.status == KinetoChoiceStatus.selected);
    return WabPanel(
      title: character.characterName,
      trailing: WabStatusBadge(
        character.locked
            ? character.stale.isEmpty
                ? 'Voice locked · ${character.lockedVoiceName}'
                : 'Voice locked · changed since'
            : character.auditions.isEmpty
                ? 'No voice yet'
                : 'Auditioned',
        key: ValueKey<String>('voice-state-$id'),
        kind: character.locked
            ? character.stale.isEmpty
                ? WabBadgeKind.done
                : WabBadgeKind.warning
            : WabBadgeKind.neutral,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          if (character.stale.isNotEmpty) Text('Changed since the voice was locked: ${character.stale.join(', ')}'),
          if (character.unresolved)
            const Text('An audition may have been charged without its audio arriving.'),
          for (final audition in character.auditions)
            Padding(
              padding: const EdgeInsets.only(top: 8),
              child: Wrap(
                spacing: 8,
                runSpacing: 4,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: <Widget>[
                  Text(
                    '${audition.voiceName}${audition.emotion == null ? '' : ' · ${audition.emotion}'}'
                    ' · ${formatSeconds(audition.durationMillis)} · “${audition.line}”',
                  ),
                  WabStatusBadge(
                    audition.id == character.lockedAudition ? 'Locked' : audition.status.wireName,
                    kind: audition.status == KinetoChoiceStatus.selected ? WabBadgeKind.done : WabBadgeKind.neutral,
                  ),
                  KinetoSoundButton(key: ValueKey<String>('play-${audition.id}'), path: audition.mediaPath),
                  if (!character.locked) ...<Widget>[
                    TextButton(
                      key: ValueKey<String>('select-${audition.id}'),
                      onPressed: widget.enabled && audition.status != KinetoChoiceStatus.selected
                          ? () => widget.onSelect(audition.id)
                          : null,
                      child: const Text('Choose'),
                    ),
                    TextButton(
                      key: ValueKey<String>('reject-${audition.id}'),
                      onPressed: widget.enabled && audition.status == KinetoChoiceStatus.candidate
                          ? () => widget.onReject(audition.id)
                          : null,
                      child: const Text('Reject'),
                    ),
                  ],
                ],
              ),
            ),
          if (!character.locked && selected.isNotEmpty) ...<Widget>[
            const SizedBox(height: 8),
            KinetoButton(
              key: ValueKey<String>('lock-voice-$id'),
              onPressed: widget.enabled ? widget.onLock : null,
              child: Text('Lock ${selected.first.voiceName}'),
            ),
          ],
          if (!character.locked) ...<Widget>[
            const SizedBox(height: 12),
            if (widget.catalog.isEmpty)
              const Text('List the provider\'s voices to choose some to audition.')
            else ...<Widget>[
              Wrap(
                spacing: 8,
                runSpacing: 8,
                children: <Widget>[
                  for (final voice in _ordered.take(24))
                    Tooltip(
                      message: _reason(voice.id) ?? voice.description,
                      child: FilterChip(
                        key: ValueKey<String>('choose-$id-${voice.id}'),
                        label: Text(_reason(voice.id) == null ? voice.name : '★ ${voice.name}'),
                        selected: _chosen.contains(voice.id),
                        onSelected: widget.enabled
                            ? (on) => setState(() => on ? _chosen.add(voice.id) : _chosen.remove(voice.id))
                            : null,
                      ),
                    ),
                ],
              ),
              const SizedBox(height: 8),
              TextField(
                key: ValueKey<String>('audition-line-$id'),
                controller: _line,
                onChanged: (_) => setState(() {}),
                decoration: wabInputDecoration(hintText: 'Line to say (default: “${character.defaultLine}”)'),
              ),
              const SizedBox(height: 8),
              Wrap(
                spacing: 12,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: <Widget>[
                  DropdownButton<KinetoEmotion?>(
                    key: ValueKey<String>('audition-delivery-$id'),
                    value: _delivery,
                    hint: const Text('Own delivery'),
                    items: <DropdownMenuItem<KinetoEmotion?>>[
                      const DropdownMenuItem<KinetoEmotion?>(child: Text('Own delivery')),
                      for (final emotion in KinetoEmotion.values)
                        DropdownMenuItem<KinetoEmotion?>(value: emotion, child: Text(emotion.label)),
                    ],
                    onChanged: widget.enabled ? (value) => setState(() => _delivery = value) : null,
                  ),
                  KinetoButton(
                    key: ValueKey<String>('audition-$id'),
                    onPressed: widget.enabled && _chosen.isNotEmpty
                        ? () => widget.onAudition(
                              <KinetoVoiceChoice>[
                                for (final voice in widget.catalog)
                                  if (_chosen.contains(voice.id))
                                    KinetoVoiceChoice(voiceId: voice.id, name: voice.name, emotion: _delivery),
                              ],
                              _line.text.trim().isEmpty ? null : _line.text.trim(),
                            )
                        : null,
                    child: Text(
                      _chosen.isEmpty
                          ? 'Audition'
                          : 'Audition ${_chosen.length} voice${_chosen.length == 1 ? '' : 's'} · $cost',
                    ),
                  ),
                ],
              ),
            ],
          ],
        ],
      ),
    );
  }
}

/// One line: takes to hear, one to choose and lock.
class LineDubbingCard extends StatefulWidget {
  const LineDubbingCard({
    required this.line,
    required this.speaker,
    required this.voiceLocked,
    required this.speechCost,
    required this.enabled,
    required this.onDub,
    required this.onSelect,
    required this.onReject,
    required this.onLock,
    super.key,
  });

  final KinetoLine line;
  final String speaker;

  /// The speaker's voice is locked, which dubbing waits for.
  final bool voiceLocked;
  final SpeechCost speechCost;
  final bool enabled;
  final ValueChanged<DubRequest> onDub;
  final ValueChanged<String> onSelect;
  final ValueChanged<String> onReject;
  final VoidCallback onLock;

  @override
  State<LineDubbingCard> createState() => _LineDubbingCardState();
}

class _LineDubbingCardState extends State<LineDubbingCard> {
  final TextEditingController _note = TextEditingController();
  int _takes = 1;
  KinetoEmotion? _delivery;

  @override
  void dispose() {
    _note.dispose();
    super.dispose();
  }

  String get _key => '${widget.line.sceneId}-${widget.line.number}';

  @override
  Widget build(BuildContext context) {
    final line = widget.line;
    final cost = line.takeCostMicros == null ? null : line.takeCostMicros! * _takes;
    final selected = line.takes.where((take) => take.status == KinetoChoiceStatus.selected);
    final length = line.durationSource == KinetoDurationSource.measured
        ? formatSeconds(line.durationMillis)
        : '≈ ${formatSeconds(line.durationMillis)}';
    return WabPanel(
      title: '${widget.speaker} · line ${line.number}',
      trailing: WabStatusBadge(
        line.locked
            ? line.stale.isEmpty
                ? 'Locked · $length'
                : 'Locked · changed since'
            : line.takes.isEmpty
                ? 'Not dubbed · $length'
                : '${line.takes.length} takes',
        key: ValueKey<String>('line-state-$_key'),
        kind: line.locked
            ? line.stale.isEmpty
                ? WabBadgeKind.done
                : WabBadgeKind.warning
            : WabBadgeKind.neutral,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Text('${line.parenthetical == null ? '' : '(${line.parenthetical}) '}“${line.text}”'),
          if (line.stale.isNotEmpty) Text('Changed since the take was locked: ${line.stale.join(', ')}'),
          if (line.unresolved) const Text('A take may have been charged without its audio arriving.'),
          for (final take in line.takes)
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: Wrap(
                spacing: 8,
                runSpacing: 4,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: <Widget>[
                  Text(
                    '${formatSeconds(take.durationMillis)}'
                    '${take.emotion == null ? '' : ' · ${take.emotion}'}'
                    '${take.note == null ? '' : ' · ${take.note}'}',
                  ),
                  WabStatusBadge(
                    take.id == line.lockedTake ? 'Locked' : take.status.wireName,
                    kind: take.status == KinetoChoiceStatus.selected ? WabBadgeKind.done : WabBadgeKind.neutral,
                  ),
                  KinetoSoundButton(key: ValueKey<String>('play-${take.id}'), path: take.mediaPath),
                  if (!line.locked) ...<Widget>[
                    TextButton(
                      key: ValueKey<String>('select-${take.id}'),
                      onPressed: widget.enabled && take.status != KinetoChoiceStatus.selected
                          ? () => widget.onSelect(take.id)
                          : null,
                      child: const Text('Choose'),
                    ),
                    TextButton(
                      key: ValueKey<String>('reject-${take.id}'),
                      onPressed: widget.enabled && take.status == KinetoChoiceStatus.candidate
                          ? () => widget.onReject(take.id)
                          : null,
                      child: const Text('Reject'),
                    ),
                  ],
                ],
              ),
            ),
          if (!line.locked && selected.isNotEmpty) ...<Widget>[
            const SizedBox(height: 8),
            KinetoButton(
              key: ValueKey<String>('lock-line-$_key'),
              onPressed: widget.enabled ? widget.onLock : null,
              child: const Text('Lock the chosen take'),
            ),
          ],
          if (!line.locked) ...<Widget>[
            const SizedBox(height: 8),
            if (!widget.voiceLocked)
              Text('Lock ${widget.speaker}\'s voice first; every take is spoken in it.')
            else ...<Widget>[
              TextField(
                key: ValueKey<String>('dub-note-$_key'),
                controller: _note,
                decoration: wabInputDecoration(hintText: 'Note for the takes (kept with them)'),
              ),
              const SizedBox(height: 8),
              Wrap(
                spacing: 12,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: <Widget>[
                  DropdownButton<int>(
                    key: ValueKey<String>('dub-takes-$_key'),
                    value: _takes,
                    items: <DropdownMenuItem<int>>[
                      for (final count in takeCounts)
                        DropdownMenuItem<int>(value: count, child: Text('$count take${count == 1 ? '' : 's'}')),
                    ],
                    onChanged: widget.enabled ? (value) => setState(() => _takes = value ?? 1) : null,
                  ),
                  DropdownButton<KinetoEmotion?>(
                    key: ValueKey<String>('dub-delivery-$_key'),
                    value: _delivery,
                    hint: const Text("Screenplay's delivery"),
                    items: <DropdownMenuItem<KinetoEmotion?>>[
                      const DropdownMenuItem<KinetoEmotion?>(child: Text("Screenplay's delivery")),
                      for (final emotion in KinetoEmotion.values)
                        DropdownMenuItem<KinetoEmotion?>(value: emotion, child: Text(emotion.label)),
                    ],
                    onChanged: widget.enabled ? (value) => setState(() => _delivery = value) : null,
                  ),
                  KinetoButton(
                    key: ValueKey<String>('dub-$_key'),
                    onPressed: widget.enabled
                        ? () => widget.onDub(
                              DubRequest(
                                takes: _takes,
                                delivery: _delivery,
                                note: _note.text.trim().isEmpty ? null : _note.text.trim(),
                              ),
                            )
                        : null,
                    child: Text('Dub $_takes take${_takes == 1 ? '' : 's'} · ${speechCostLabel(cost)}'),
                  ),
                ],
              ),
            ],
          ],
        ],
      ),
    );
  }
}
