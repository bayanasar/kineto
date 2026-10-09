import 'package:flutter/material.dart';
import 'package:wabisabi/wabisabi.dart';

import 'kineto_controls.dart';
import 'native/kineto_engine.dart';

/// What the bible can be told about: the world, and each entry.
List<(String, String)> referenceTargets(KinetoBible? bible) => <(String, String)>[
      (kinetoReferenceWorld, 'The world'),
      if (bible != null)
        for (final entry in <KinetoBibleEntry>[...bible.characters, ...bible.locations, ...bible.props])
          (kinetoReferenceTarget(entry), '${entry.name} · ${entry.kind.wireName}'),
    ];

/// References from MCP tools (ADR-0007): asking a tool the director chose,
/// and what was kept, attached, edited or removed. A tool's answer is only
/// ever material for the bible.
class ReferencesPanel extends StatefulWidget {
  const ReferencesPanel({
    required this.references,
    required this.targets,
    required this.enabled,
    required this.onLoadTools,
    required this.onAsk,
    required this.onEdit,
    required this.onAttach,
    required this.onRemove,
    this.servers,
    this.serversProblem,
    this.loadingTools = false,
    super.key,
  });

  final List<KinetoReference> references;

  /// What a reference can be about, as `(target, label)`.
  final List<(String, String)> targets;
  final bool enabled;

  /// The registered servers and their tools; null until loaded.
  final List<KinetoMcpServer>? servers;
  final String? serversProblem;
  final bool loadingTools;
  final VoidCallback onLoadTools;
  final void Function(String server, String tool, List<KinetoToolArgument> arguments, List<String> targets) onAsk;
  final void Function(String reference, String text) onEdit;
  final void Function(String reference, String target, bool attach) onAttach;
  final ValueChanged<String> onRemove;

  @override
  State<ReferencesPanel> createState() => _ReferencesPanelState();
}

class _ReferencesPanelState extends State<ReferencesPanel> {
  String? _server;
  String? _tool;
  final Map<String, TextEditingController> _fields = <String, TextEditingController>{};
  final Map<String, bool> _flags = <String, bool>{};
  final Set<String> _about = <String>{kinetoReferenceWorld};
  String? _editing;
  final TextEditingController _edit = TextEditingController();

  @override
  void dispose() {
    for (final field in _fields.values) {
      field.dispose();
    }
    _edit.dispose();
    super.dispose();
  }

  List<KinetoMcpServer> get _usable => <KinetoMcpServer>[
        for (final server in widget.servers ?? const <KinetoMcpServer>[])
          if (server.enabled && (server.tools?.isNotEmpty ?? false)) server,
      ];

  KinetoMcpServer? get _chosenServer {
    for (final server in _usable) {
      if (server.id == _server) return server;
    }
    return _usable.isEmpty ? null : _usable.first;
  }

  KinetoMcpTool? get _chosenTool {
    final tools = _chosenServer?.tools ?? const <KinetoMcpTool>[];
    for (final tool in tools) {
      if (tool.name == _tool) return tool;
    }
    return tools.isEmpty ? null : tools.first;
  }

  TextEditingController _field(String name) => _fields.putIfAbsent(name, TextEditingController.new);

  List<KinetoToolArgument>? _arguments(KinetoMcpTool tool) {
    final arguments = <KinetoToolArgument>[];
    for (final parameter in tool.parameters) {
      final kind = parameter.kind;
      if (kind == null) {
        if (parameter.required) return null;
        continue;
      }
      if (kind == KinetoToolArgumentKind.flag) {
        final flag = _flags[parameter.name];
        if (flag != null) arguments.add(KinetoToolArgument(parameter.name, '$flag', kind: kind));
        continue;
      }
      final value = _field(parameter.name).text.trim();
      if (value.isEmpty) {
        if (parameter.required) return null;
        continue;
      }
      if (kind == KinetoToolArgumentKind.number && num.tryParse(value) == null) return null;
      arguments.add(KinetoToolArgument(parameter.name, value, kind: kind));
    }
    return arguments;
  }

  @override
  Widget build(BuildContext context) {
    final references = widget.references;
    return WabPanel(
      title: 'References',
      trailing: WabStatusBadge(
        references.isEmpty ? 'None yet' : '${references.length} kept',
        kind: WabBadgeKind.neutral,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          const Text(
            'Outside material for the bible, from the MCP tools you registered: wardrobe notes, a real '
            'place, a studio\'s character sheet. The bible reads the references attached to the world '
            'or to an entry the next time it is written, and says when one changed since. What a tool '
            'answers is only ever material: nothing in it is acted on.',
          ),
          const SizedBox(height: 12),
          _composer(context),
          for (final reference in references) ...<Widget>[
            const SizedBox(height: 12),
            _reference(context, reference),
          ],
        ],
      ),
    );
  }

  Widget _composer(BuildContext context) {
    final servers = widget.servers;
    if (servers == null) {
      return Wrap(
        spacing: 12,
        runSpacing: 8,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: <Widget>[
          KinetoButton(
            key: const ValueKey<String>('load-tools'),
            onPressed: widget.loadingTools ? null : widget.onLoadTools,
            child: Text(widget.loadingTools ? 'Asking the servers for their tools' : 'Show the tools I can ask'),
          ),
          if (widget.serversProblem != null) Text(widget.serversProblem!),
        ],
      );
    }
    final server = _chosenServer;
    final tool = _chosenTool;
    if (server == null || tool == null) {
      return const Text(
        'No registered MCP server offers a tool it allows. Register one in mcp.toml; see Settings.',
      );
    }
    final arguments = _arguments(tool);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: <Widget>[
        Wrap(
          spacing: 12,
          runSpacing: 8,
          crossAxisAlignment: WrapCrossAlignment.center,
          children: <Widget>[
            SizedBox(
              width: 300,
              child: WabDropdown<String>(
                key: const ValueKey<String>('reference-server'),
                value: server.id,
                items: <String, String>{for (final usable in _usable) usable.id: usable.id},
                onChanged: (id) => setState(() {
                  _server = id;
                  _tool = null;
                }),
              ),
            ),
            SizedBox(
              width: 360,
              child: WabDropdown<String>(
                key: const ValueKey<String>('reference-tool'),
                value: tool.name,
                items: <String, String>{
                  for (final offered in server.tools ?? const <KinetoMcpTool>[]) offered.name: offered.name,
                },
                onChanged: (name) => setState(() => _tool = name),
              ),
            ),
          ],
        ),
        if (tool.description.isNotEmpty) ...<Widget>[const SizedBox(height: 4), Text(tool.description)],
        for (final parameter in tool.parameters) ...<Widget>[
          const SizedBox(height: 8),
          if (parameter.kind == null)
            Text('${parameter.name} cannot be given here${parameter.required ? ', and the tool needs it' : ''}.')
          else if (parameter.kind == KinetoToolArgumentKind.flag)
            WabCheckbox(
              key: ValueKey<String>('argument-${parameter.name}'),
              value: _flags[parameter.name] ?? false,
              onChanged: (value) => setState(() => _flags[parameter.name] = value ?? false),
              label: parameter.name,
            )
          else
            TextField(
              key: ValueKey<String>('argument-${parameter.name}'),
              controller: _field(parameter.name),
              onChanged: (_) => setState(() {}),
              decoration: wabInputDecoration(
                hintText: '${parameter.name}${parameter.required ? '' : ' (optional)'}'
                    '${parameter.description.isEmpty ? '' : ' · ${parameter.description}'}',
              ),
            ),
        ],
        const SizedBox(height: 8),
        Wrap(
          spacing: 16,
          runSpacing: 4,
          children: <Widget>[
            const Text('About'),
            for (final (target, label) in widget.targets)
              WabCheckbox(
                key: ValueKey<String>('ask-about-$target'),
                value: _about.contains(target),
                onChanged: (value) => setState(() => value ?? false ? _about.add(target) : _about.remove(target)),
                label: label,
              ),
          ],
        ),
        const SizedBox(height: 8),
        Align(
          alignment: Alignment.centerLeft,
          child: KinetoButton(
            key: const ValueKey<String>('ask-reference'),
            primary: true,
            onPressed: widget.enabled && arguments != null
                ? () => widget.onAsk(
                      server.id,
                      tool.name,
                      arguments,
                      <String>[
                        for (final (target, _) in widget.targets)
                          if (_about.contains(target)) target,
                      ],
                    )
                : null,
            child: Text('Ask ${server.id} · ${tool.name}'),
          ),
        ),
      ],
    );
  }

  Widget _reference(BuildContext context, KinetoReference reference) {
    final editing = _editing == reference.id;
    final about = reference.attachedTo.map(describeReferenceTarget).join(', ');
    return WabPanel(
      key: ValueKey<String>('reference-${reference.id}'),
      title: '${reference.id} · ${reference.server}/${reference.tool}',
      trailing: WabStatusBadge(
        reference.attachedTo.isEmpty ? 'Read by nothing' : 'About $about',
        kind: reference.attachedTo.isEmpty ? WabBadgeKind.neutral : WabBadgeKind.done,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          if (reference.arguments.isNotEmpty) Text('Asked with ${reference.arguments.join('; ')}'),
          const SizedBox(height: 4),
          if (editing)
            TextField(
              key: ValueKey<String>('reference-text-${reference.id}'),
              controller: _edit,
              maxLines: null,
              minLines: 3,
              decoration: wabInputDecoration(hintText: 'The reference, in your words'),
            )
          else
            SelectableText(reference.text),
          const SizedBox(height: 8),
          Wrap(
            spacing: 16,
            runSpacing: 4,
            children: <Widget>[
              for (final (target, label) in widget.targets)
                WabCheckbox(
                  key: ValueKey<String>('attach-${reference.id}-$target'),
                  value: reference.attachedTo.contains(target),
                  onChanged: widget.enabled && !editing
                      ? (value) => widget.onAttach(reference.id, target, value ?? false)
                      : null,
                  label: label,
                ),
            ],
          ),
          const SizedBox(height: 8),
          Wrap(
            spacing: 12,
            runSpacing: 8,
            children: editing
                ? <Widget>[
                    KinetoButton(
                      key: ValueKey<String>('save-reference-${reference.id}'),
                      primary: true,
                      onPressed: widget.enabled && _edit.text.trim().isNotEmpty
                          ? () {
                              widget.onEdit(reference.id, _edit.text);
                              setState(() => _editing = null);
                            }
                          : null,
                      child: const Text('Save'),
                    ),
                    KinetoButton(
                      key: ValueKey<String>('cancel-reference-${reference.id}'),
                      onPressed: () => setState(() => _editing = null),
                      child: const Text('Cancel'),
                    ),
                  ]
                : <Widget>[
                    KinetoButton(
                      key: ValueKey<String>('edit-reference-${reference.id}'),
                      onPressed: widget.enabled
                          ? () => setState(() {
                                _editing = reference.id;
                                _edit.text = reference.text;
                              })
                          : null,
                      child: const Text('Edit'),
                    ),
                    KinetoButton(
                      key: ValueKey<String>('remove-reference-${reference.id}'),
                      onPressed: widget.enabled ? () => _confirmRemove(context, reference) : null,
                      child: const Text('Remove'),
                    ),
                  ],
          ),
        ],
      ),
    );
  }

  Future<void> _confirmRemove(BuildContext context, KinetoReference reference) async {
    final remove = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text('Remove ${reference.id}?'),
        content: const Text(
          'Its text is gone from the project. A bible that read it says so until it is written again.',
        ),
        actions: <Widget>[
          TextButton(onPressed: () => Navigator.pop(context, false), child: const Text('Keep it')),
          TextButton(
            key: const ValueKey<String>('remove-reference-confirm'),
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Remove'),
          ),
        ],
      ),
    );
    if (remove ?? false) widget.onRemove(reference.id);
  }
}
