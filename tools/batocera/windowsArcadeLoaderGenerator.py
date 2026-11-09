from __future__ import annotations

import json
import logging
import os
from pathlib import Path
from typing import TYPE_CHECKING, Final

from configgen import Command
from configgen.batoceraPaths import BATOCERA_ES_DIR, CONFIGS, USER_ES_DIR, configure_emulator, mkdir_if_not_exists
from configgen.controller import Controller, generate_sdl_game_controller_config
from configgen.input import Input
from configgen.generators.Generator import Generator
from configgen.utils.gamescope import gamescope_args

if TYPE_CHECKING:
    from configgen.types import HotkeysContext

_logger = logging.getLogger(__name__)

# Windows arcade games (Taito Type X / X2, NESiCAxLive, ...) through the Windows arcade
# loader: arcade-launcher runs the game in Wine with its system's I/O emulated (JVS board,
# NESiCA card reader...) and maps the pads, keyboards and guns itself (SDL and evdev), as
# the game's profile says (systemprofiles/<system>/<gameid>.yaml).
#
# The rom is a game dump directory holding <gameid>.windowsloader (the executable path
# relative to the dump root): ES lists that file, the game id selects the profile. Or the
# dump packed as a .squashfs image: configgen mounts it and hands the mounted directory;
# writesToRom() gives it a writable overlay kept in /userdata/saves/<system>/<image name>
# (the game's save data in the dump's WindowsLoader folder).
#
# The ES options are written as an extra profile layer (--profile), merged over the game's
# profile: only the options set in ES, so a game's own setting (e.g. KOF Maximum Impact
# Regulation A's dxvk: false) stays unless the user picks another.
#
# GAMESCOPE options: the launcher starts gamescope around wine (its inputs and force feedback
# stay outside), so configgen does not wrap the command (gamescope.py SELF_GAMESCOPE); they
# go in the layer's gamescope: settings, over the game profile's (Gundam Spirits of Zeon 2
# players: 1280x480). The game resolution stays the profile's unless set in ES. Any GAMESCOPE
# option set enables it, unless GAMESCOPE ENABLE is Off.
#
# Layout of the emulator directory:
#   arcade-launcher, payloads/ (wal_*.dll, wal-loader.exe), systemprofiles/,
#   launcher.yaml (machine settings: runners_dir, payloads_dir), wine-prefix/ (created)
# The Wine runner (GE-Proton with the loader's hotfixes) is in /userdata/system/wine/custom.
#
# The GUI (applications menu, arcade-launcher-gui) comes through here too, with the rom
# "config": SDL then gets the pads' ES mappings as in games (without them, pads SDL3 does not
# know, e.g. Xbox ones without HIDAPI, are plain joysticks).

WAL_DIR: Final = Path('/userdata/system/rgs/emulators/windows-arcade-loader')
WAL_CONFIG: Final = CONFIGS / 'windows-arcade-loader'


_GAMESCOPE_SWITCHES: Final = {'gamescope_hdr', 'gamescope_hdr_itm_enabled'}


def _es_pads() -> list[Controller]:
    # outside a game ES passes no -pX controllers: every pad configured in ES (user file first)
    import xml.etree.ElementTree as ET

    pads: dict[str, Controller] = {}
    for conffile in (USER_ES_DIR / 'es_input.cfg', BATOCERA_ES_DIR / 'es_input.cfg'):
        if not conffile.exists():
            continue
        for element in ET.parse(conffile).getroot().iterfind('./inputConfig[@type="joystick"]'):
            guid = element.get('deviceGUID') or ''
            name = element.get('deviceName') or ''
            if not guid or guid in pads:
                continue
            pads[guid] = Controller(
                name=name, type='joystick', guid=guid, player_number=len(pads) + 1, index=len(pads),
                real_name=name, device_path='', button_count=0, hat_count=0, axis_count=0,
                inputs_=Input.from_parent_element(element),
            )
    return list(pads.values())


def _player_pads(controllers) -> str:
    # ES's player order (P1 first) by evdev node: the launcher gives the players in it, under
    # the GUI's priority list (userprofiles/players.yaml)
    return ';'.join(c.device_path for c in controllers if c.device_path)


def _yaml_scalar(value: str | bool) -> str:
    if isinstance(value, bool):
        return 'true' if value else 'false'
    return f'"{value}"'


class WindowsArcadeLoaderGenerator(Generator):

    def getHotkeysContext(self) -> HotkeysContext:
        # Exit: the launcher stops the game and quits on SIGTERM. Coin: P1 coin (key 5).
        return {
            'name': 'windows-arcade-loader',
            'keys': {'exit': '/usr/bin/pkill -TERM -x arcade-launcher', 'coin': 'KEY_5'},
        }

    def writesToRom(self, config) -> bool:
        return True

    # Gun games: the payload draws the Sinden frame itself (WAL_SINDEN_BORDER), so configgen
    # draws neither its gun border overlay nor a MangoHUD bezel. Read after generate().
    _guns_borders_internal = False

    def supportsInternalBezels(self) -> bool:
        return self._guns_borders_internal

    def generate(self, system, rom, playersControllers, metadata, guns, wheels, gameResolution):
        if configure_emulator(rom):
            os.chdir(WAL_DIR)
            return Command.Command(
                array=[str(WAL_DIR / 'arcade-launcher'), 'gui', '--root', str(WAL_DIR)],
                env={
                    'SDL_GAMECONTROLLERCONFIG': generate_sdl_game_controller_config(playersControllers or _es_pads()),
                    'SDL_JOYSTICK_HIDAPI': '0',
                    'WAL_PLAYER_PADS': _player_pads(playersControllers or []),
                },
            )

        mkdir_if_not_exists(WAL_CONFIG)
        self._guns_borders_internal = bool(getattr(system.config, 'use_guns', False))

        # ES options -> profile keys (top level or under input:), set ones only.
        top: dict[str, str] = {}
        # Batocera quits the game itself (hotkey + start: the hotkeys context's exit), no
        # internal pad combo (back + start: coin + start on arcade panels with coin on select)
        input_keys: dict[str, str] = {'exit_combo': '[]'}
        reshade = system.config.get('wal_reshade')
        if reshade in ('0', '1'):
            # rotation / bezel of the dumps that ship a ReShade setup (vertical games)
            top['reshade'] = _yaml_scalar(reshade == '1')
        renderer = system.config.get('wal_renderer')
        if renderer in ('dxvk', 'wined3d'):
            top['dxvk'] = _yaml_scalar(renderer == 'dxvk')
        mouse_guns = system.config.get('wal_mouse_guns')
        if mouse_guns in ('0', '1'):
            input_keys['guns_mouse'] = _yaml_scalar(mouse_guns == '1')
        env_keys: dict[str, str] = {}
        # Native Sinden frame: Batocera's system/game border settings, with an explicit
        # loader override when chosen. Auto still gates activation on the game's gun profile.
        border_mode = system.config.get('bordersmode') or 'auto'
        if border_mode == 'auto':
            border_mode = system.config.get('controllers.guns.bordersmode') or 'auto'
        border = {'hidden': '0', 'force': '1'}.get(border_mode, 'auto')
        override = system.config.get('wal_sinden_border')
        if override in ('auto', '0', '1'):
            border = override
        env_keys['WAL_SINDEN_BORDER'] = json.dumps(border)
        env_keys['WAL_SINDEN_BORDER_SIZE'] = json.dumps(str(system.config.get('wal_sinden_border_size') or system.config.get('controllers.guns.borderssize') or 'medium'))
        env_keys['WAL_SINDEN_BORDER_COLOR'] = json.dumps(str(system.config.get('wal_sinden_border_color') or system.config.get('controllers.guns.borderscolor') or 'white'))
        # The frontend knows virtual Sinden devices too; sysfs detection remains the fallback.
        env_keys['WAL_SINDEN_DETECTED'] = json.dumps('1' if any(g.needs_borders for g in guns) else '0')
        keep_aspect = system.config.get('keep_aspect_ratio')
        if keep_aspect in ('on', 'off'):
            # games whose picture the payload scales to the screen (Gashaaaan)
            env_keys['WAL_KEEP_ASPECT_RATIO'] = _yaml_scalar('1' if keep_aspect == 'on' else '0')
        crosshair = system.config.get('wal_crosshair')
        if crosshair in ('0', '1'):
            # the common crosshair option of the games able to show crosshairs
            env_keys['WAL_CROSSHAIR'] = _yaml_scalar(crosshair)
        freeplay = system.config.get('wal_freeplay')
        if freeplay in ('0', '1'):
            # the common free play option (the loader's default: on)
            env_keys['WAL_FREEPLAY'] = _yaml_scalar(freeplay)
        keyboard = system.config.get('wal_keyboard')
        if keyboard in ('0', '1'):
            input_keys['keyboard_enabled'] = _yaml_scalar(keyboard == '1')

        gamescope_keys: dict[str, str] = {}
        gamescope = system.config.get('gamescope')
        # any GAMESCOPE option set turns it on (unless ENABLE is set to Off)
        def gamescope_option_set(key: str) -> bool:
            value = str(system.config.get(key) or '')
            # switches (HDR...) set to Off do not count
            return value != '' and not (key in _GAMESCOPE_SWITCHES and value == '0')

        if gamescope != '0' and any(k.startswith('gamescope_') and gamescope_option_set(k) for k in system.config.keys()):
            gamescope = '1'
        if gamescope in ('0', '1'):
            gamescope_keys['enabled'] = _yaml_scalar(gamescope == '1')
        if gamescope == '1':
            nested = system.config.get('gamescope_nested_resolution')
            if nested:
                width, _, height = nested.partition('x')
                gamescope_keys['width'] = str(int(width))
                gamescope_keys['height'] = str(int(height))
            # a JSON list is a YAML flow sequence
            gamescope_keys['args'] = json.dumps(gamescope_args(system, gameResolution, nested=False))

        lines = [f'{key}: {value}' for key, value in top.items()]
        if input_keys:
            lines.append('input:')
            lines.extend(f'  {key}: {value}' for key, value in input_keys.items())
        if env_keys:
            lines.append('env:')
            lines.extend(f'  {key}: {value}' for key, value in env_keys.items())
        if gamescope_keys:
            lines.append('gamescope:')
            lines.extend(f'  {key}: {value}' for key, value in gamescope_keys.items())
        layer = WAL_CONFIG / 'es-options.yaml'
        layer.write_text('\n'.join(lines) + '\n')
        _logger.debug('windows-arcade-loader options %s:\n%s', layer, '\n'.join(lines))

        command = [str(WAL_DIR / 'arcade-launcher'), 'run', str(rom), '--root', str(WAL_DIR), '--profile', str(layer)]
        os.chdir(WAL_DIR)
        return Command.Command(
            array=command,
            env={
                'SDL_GAMECONTROLLERCONFIG': generate_sdl_game_controller_config(playersControllers),
                'SDL_JOYSTICK_HIDAPI': '0',
                'WAL_PLAYER_PADS': _player_pads(playersControllers),
            },
        )
