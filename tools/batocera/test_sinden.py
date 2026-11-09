"""Exercise the generator's real YAML output without a Batocera installation."""
import ast
import json
from pathlib import Path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest


class SindenOptions(unittest.TestCase):
    def generate(self, settings, guns=()):
        source = Path(__file__).with_name('windowsArcadeLoaderGenerator.py').read_text()
        tree = ast.parse(source)
        tree.body = [n for n in tree.body if isinstance(n, (ast.FunctionDef, ast.ClassDef))]
        with TemporaryDirectory() as tmp:
            root = Path(tmp)
            namespace = dict(
                Generator=object, HotkeysContext=dict, Controller=object, json=json,
                WAL_DIR=root, WAL_CONFIG=root, configure_emulator=lambda _: False,
                mkdir_if_not_exists=lambda _: None, os=SimpleNamespace(chdir=lambda _: None),
                generate_sdl_game_controller_config=lambda _: '',
                Command=SimpleNamespace(Command=lambda **kwargs: kwargs),
                _GAMESCOPE_SWITCHES=set(),
                _logger=SimpleNamespace(debug=lambda *_: None),
            )
            exec(compile(ast.unparse(tree), '<generator>', 'exec', flags=__import__('__future__').annotations.compiler_flag), namespace)
            self.generator = namespace['WindowsArcadeLoaderGenerator']()
            self.generator.generate(SimpleNamespace(config=settings), 'game.windowsloader', [], {}, guns, {}, {})
            return (root / 'es-options.yaml').read_text()

    def test_system_settings_are_passed(self):
        output = self.generate({'controllers.guns.borderssize': 'big', 'controllers.guns.borderscolor': 'green'}, [SimpleNamespace(needs_borders=True)])
        self.assertIn('WAL_SINDEN_BORDER: "auto"', output)
        self.assertIn('WAL_SINDEN_BORDER_SIZE: "big"', output)
        self.assertIn('WAL_SINDEN_BORDER_COLOR: "green"', output)
        self.assertIn('WAL_SINDEN_DETECTED: "1"', output)

    def test_manual_override_and_rgb(self):
        output = self.generate({'wal_sinden_border': '1', 'wal_sinden_border_size': '1.5', 'wal_sinden_border_color': '#123456'})
        for expected in ['BORDER: "1"', 'SIZE: "1.5"', 'COLOR: "#123456"']:
            self.assertIn(expected, output)

    def test_mode_precedence_and_defaults(self):
        self.assertIn('BORDER: "0"', self.generate({'controllers.guns.bordersmode': 'force', 'bordersmode': 'hidden'}))
        self.assertIn('BORDER: "1"', self.generate({'controllers.guns.bordersmode': 'force'}))
        self.assertIn('BORDER: "auto"', self.generate({}))
        self.assertIn('SIZE: "medium"', self.generate({}))
        self.assertIn('COLOR: "white"', self.generate({}))

    def test_configgen_borders_skipped_for_gun_games(self):
        class Config(dict):
            use_guns = True
        self.generate(Config())
        self.assertTrue(self.generator.supportsInternalBezels())
        self.generate({})
        self.assertFalse(self.generator.supportsInternalBezels())


if __name__ == '__main__':
    unittest.main()
