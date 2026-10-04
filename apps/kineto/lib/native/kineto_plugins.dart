part of 'kineto_engine.dart';

@Native<Int32 Function(Pointer<Pointer<Void>>)>(symbol: 'kineto_plugins_load')
external int _kinetoPluginsLoad(Pointer<Pointer<Void>> outPlugins);

@Native<Uint32 Function(Pointer<Void>)>(symbol: 'kineto_plugins_count', isLeaf: true)
external int _kinetoPluginsCount(Pointer<Void> plugins);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Pointer<Uint64>)>(
  symbol: 'kineto_plugins_value',
  isLeaf: true,
)
external int _kinetoPluginsValue(Pointer<Void> plugins, int index, int field, Pointer<Uint64> out);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Pointer<Uint8>, Uint64, Pointer<Uint64>)>(
  symbol: 'kineto_plugins_text_copy',
  isLeaf: true,
)
external int _kinetoPluginsTextCopy(
  Pointer<Void> plugins,
  int index,
  int field,
  Pointer<Uint8> buffer,
  int capacity,
  Pointer<Uint64> length,
);

@Native<Void Function(Pointer<Void>)>(symbol: 'kineto_plugins_destroy')
external void _kinetoPluginsDestroy(Pointer<Void> plugins);

/// The number fields of a plugin these bindings read.
const Set<String> kinetoPluginValueFields = <String>{
  'enabled',
  'described',
  'selected',
  'makes_images',
  'speaks',
  'identity_references',
  'composition_reference',
  'image_price_known',
  'image_price_micros',
  'speech_price_known',
  'speech_character_price_micros',
  'makes_videos',
};

/// The text fields of a plugin these bindings read.
const Set<String> kinetoPluginTextFields = <String>{
  'id',
  'name',
  'version',
  'executable',
  'secrets',
  'image_model',
  'speech_model',
  'problem',
  'aspect_ratios',
  'languages',
  'video_model',
  'clip_seconds',
};

/// What a plugin said it can do when it described itself.
final class KinetoPluginDescription {
  const KinetoPluginDescription({
    required this.name,
    required this.version,
    this.imageModel,
    this.aspectRatios = const <String>[],
    this.identityReferences = 0,
    this.compositionReference = false,
    this.imagePriceMicros,
    this.speechModel,
    this.languages = const <String>[],
    this.speechCharacterPriceMicros,
    this.videoModel,
    this.clipSeconds = const <int>[],
  });

  final String name;
  final String version;

  /// Its image model; null when it makes no images.
  final String? imageModel;

  /// Frames it makes images in, such as `16:9`.
  final List<String> aspectRatios;

  /// How many identity references one image may carry.
  final int identityReferences;

  /// Whether it takes the previous shot's keyframe as a composition
  /// reference.
  final bool compositionReference;

  /// What one image costs, in USD micros; null when it does not say.
  final int? imagePriceMicros;

  /// Its speech model; null when it does not speak.
  final String? speechModel;

  /// Languages it speaks, as ISO 639-1 codes.
  final List<String> languages;

  /// What one character of speech costs, in USD micros; null when it does
  /// not say.
  final int? speechCharacterPriceMicros;

  /// Its video model; null when it makes no video.
  final String? videoModel;

  /// Clip lengths it renders, in seconds.
  final List<int> clipSeconds;

  bool get makesImages => imageModel != null;
  bool get speaks => speechModel != null;
  bool get makesVideos => videoModel != null;
}

/// A provider plugin the user registered in `plugins.toml` (ADR-0006). Read
/// only: plugins are registered in the configuration file, never through
/// the app.
final class KinetoPlugin {
  const KinetoPlugin({
    required this.id,
    required this.executable,
    this.secrets = const <String>[],
    this.enabled = true,
    this.selected = false,
    this.description,
    this.problem = '',
  });

  final String id;

  /// The absolute path it is started from.
  final String executable;

  /// Names of the secrets it is given.
  final List<String> secrets;

  final bool enabled;

  /// Whether `KINETO_IMAGE_PROVIDER` names it.
  final bool selected;

  /// What it described; null when it is disabled or could not describe
  /// itself.
  final KinetoPluginDescription? description;

  /// Why it could not describe itself; empty otherwise.
  final String problem;

  /// What `KINETO_IMAGE_PROVIDER` names it by.
  String get selection => 'plugin:$id';
}

/// The plugins the user registered, each enabled one started once to
/// describe itself. That can take as long as a slow plugin's deadline, so
/// call it off the UI thread, as [loadKinetoPluginsInBackground] does.
List<KinetoPlugin> loadKinetoPlugins() {
  _assertAbiCompatible();
  final contract = KinetoNativeContract.instance;
  final arena = Arena();
  try {
    final out = arena<Pointer<Void>>();
    _checkProjectResult(_kinetoPluginsLoad(out));
    final handle = out.value;
    try {
      final value = arena<Uint64>();
      int number(int index, String field) {
        _checkProjectResult(_kinetoPluginsValue(handle, index, contract.code(contract.pluginValues, field), value));
        return value.value;
      }

      String text(int index, String field) {
        final code = contract.code(contract.pluginTexts, field);
        return _copyProjectUtf8(
          handle,
          (plugins, buffer, capacity, length) => _kinetoPluginsTextCopy(plugins, index, code, buffer, capacity, length),
          field,
        );
      }

      List<String> lines(int index, String field) =>
          text(index, field).split('\n').where((line) => line.isNotEmpty).toList(growable: false);

      return <KinetoPlugin>[
        for (var index = 0; index < _kinetoPluginsCount(handle); index++)
          KinetoPlugin(
            id: text(index, 'id'),
            executable: text(index, 'executable'),
            secrets: lines(index, 'secrets'),
            enabled: number(index, 'enabled') != 0,
            selected: number(index, 'selected') != 0,
            problem: text(index, 'problem'),
            description: number(index, 'described') == 0
                ? null
                : KinetoPluginDescription(
                    name: text(index, 'name'),
                    version: text(index, 'version'),
                    imageModel: number(index, 'makes_images') == 0 ? null : text(index, 'image_model'),
                    aspectRatios: lines(index, 'aspect_ratios'),
                    identityReferences: number(index, 'identity_references'),
                    compositionReference: number(index, 'composition_reference') != 0,
                    imagePriceMicros:
                        number(index, 'image_price_known') == 0 ? null : number(index, 'image_price_micros'),
                    speechModel: number(index, 'speaks') == 0 ? null : text(index, 'speech_model'),
                    languages: lines(index, 'languages'),
                    speechCharacterPriceMicros: number(index, 'speech_price_known') == 0
                        ? null
                        : number(index, 'speech_character_price_micros'),
                    videoModel: number(index, 'makes_videos') == 0 ? null : text(index, 'video_model'),
                    clipSeconds: lines(index, 'clip_seconds').map(int.parse).toList(growable: false),
                  ),
          ),
      ];
    } finally {
      _kinetoPluginsDestroy(handle);
    }
  } finally {
    arena.releaseAll();
  }
}

/// [loadKinetoPlugins] on another isolate.
Future<List<KinetoPlugin>> loadKinetoPluginsInBackground() => Isolate.run(loadKinetoPlugins);
