# Windows Arcade Loader

Runs Windows-based arcade games (Taito Type X / X2, NESiCAxLive, Global VR, Namco ES3, Tsunami)
on Linux with Wine: a Linux launcher (inputs, Wine prefix and runner) and per-system payload DLLs
emulating each cabinet's I/O, dongle and network services in the game process.

* Build: `./build.sh` (`dist/arcade-launcher`, `dist/payloads/*.dll`)
* Run: `dist/arcade-launcher run <dump>/<gameid>.windowsloader`
* Regression test: `tools/regression.py` (see its header), then `tools/gamelist.py` to refresh
  the list below
* Documentation: [docs/IMPLEMENTATION.md](docs/IMPLEMENTATION.md) (implementation),
  [DESIGN.md](DESIGN.md) (research notes)

## Games

Status: as verified by hand (docs/IMPLEMENTATION.md). Regression test: the game's latest
automated run (started, still running, a window, a picture, sound; attract mode, coin and start
after 30 s). Picture and sound checks can miss a dark or silent moment of an attract mode: a
"fail" there is to be confirmed by hand.

<!-- GAMELIST BEGIN (tools/gamelist.py) -->
| Game id | Game | System | Status | Regression test | Run |
|---|---|---|---|---|---|
| `farcry-paradise-lost` | Far Cry Paradise Lost | globalvr | not playable (user: in-game bug) | not in the test |  |
| `akai-katana-shin` | Akai Katana Shin | nesica | in game (GAME_START) | pass | 2026-10-07 |
| `aquapazza` | Aquapazza: Aquaplus Dream Match | nesica | works (user: 100%) | pass | 2026-10-08 |
| `arcana-heart-2` | Arcana Heart 2 | nesica | in game (user) | pass | 2026-10-07 |
| `arcana-heart-3-lmss` | Arcana Heart 3 Love Max Six Stars!!!!!! | nesica | in game (user, GAME_START) | pass | 2026-10-07 |
| `blazblue-central-fiction` | BlazBlue Central Fiction 2.01 | nesica | in game, NESiCA online | pass | 2026-10-07 |
| `blazblue-chronophantasma` | BlazBlue Chronophantasma 2.03 | nesica | works (user: 100%) | pass | 2026-10-09 |
| `chaos-breaker` | Chaos Breaker | nesica | in fight, music | pass | 2026-10-07 |
| `chaos-code-103` | Chaos Code: New Sign of Catastrophe 1.03 | nesica | in fight (user) | pass | 2026-10-07 |
| `chaos-code-211` | Chaos Code: New Sign of Catastrophe 2.11 | nesica | in fight (user) | pass | 2026-10-07 |
| `crimzon-clover` | Crimzon Clover | nesica | works fullscreen (user; 1280x800: pillarboxed on a 16:9 screen) | fail: picture | 2026-10-07 |
| `daemon-bride` | Daemon Bride: Additional Gain | nesica | in fight | pass | 2026-10-07 |
| `dariusburst-another-chronicle-ex` | Dariusburst Another Chronicle EX | nesica | works (user: 100%), 4 players, sound, test menu in Japanese | pass | 2026-10-08 |
| `dark-awake` | Dark Awake: The King Has No Name | nesica | in fight | pass | 2026-10-07 |
| `do-not-fall` | Do Not Fall: Run for Your Drink | nesica | works (user) | pass | 2026-10-07 |
| `dragon-dance` | Dragon Dance | nesica | works (user), smoke and sparkle effects correct with d7vk; crash on applying saved display settings fixed by a code patch | pass | 2026-10-08 |
| `elevator-action` | Elevator Action Death Parade | nesica | works (user: 100%), 4:3 | pass | 2026-10-09 |
| `en-eins-perfektewelt` | EN-Eins Perfektewelt | nesica | works (user) | pass | 2026-10-07 |
| `exception` | Exception | nesica | works fullscreen (user) | pass | 2026-10-07 |
| `gouketsuji-ichizoku` | Gouketsuji Ichizoku: Matsuri Senzo Kuyou | nesica | works (user) | pass | 2026-10-07 |
| `homura` | Homura | nesica | works (user), sound effects and music in game; attract demo silent, as under Windows (original or dump, not the loader) | pass | 2026-10-07 |
| `hyper-street-fighter-2` | Hyper Street Fighter II: The Anniversary Edition | nesica | works (user) | pass | 2026-10-07 |
| `ikaruga` | Ikaruga | nesica | in game (user) | pass | 2026-10-07 |
| `kof-2002-um` | The King of Fighters 2002 Unlimited Match | nesica | works (user) | pass | 2026-10-07 |
| `kof-98-umfe` | The King of Fighters '98 Ultimate Match Final Edition | nesica | works (user) | pass | 2026-10-07 |
| `kof-xiii-climax` | The King of Fighters XIII Climax | nesica | in game, movies | pass | 2026-10-09 |
| `magical-beat` | Magical Beat | nesica | works (user) | pass | 2026-10-07 |
| `nitroplus-blasterz` | Nitroplus Blasterz: Heroines Infinite Duel | nesica | works (user) | pass | 2026-10-07 |
| `persona-4-arena` | Persona 4 The Ultimate in Mayonaka Arena | nesica | works (user: perfect) | pass | 2026-10-07 |
| `persona-4-ultimax` | Persona 4 The Ultimax Ultra Suplex Hold | nesica | works (user) | pass | 2026-10-07 |
| `psychic-force-2012` | Psychic Force 2012 | nesica | works (user) | pass | 2026-10-07 |
| `puzzle-bobble` | Puzzle Bobble | nesica | works (user) | pass | 2026-10-07 |
| `raiden-3` | Raiden III | nesica | in game (user); intro movie black | pass | 2026-10-07 |
| `raiden-4` | Raiden IV | nesica | in game (user), intro movie (with GE-Proton's winedmo MPEG sequence header fix, after 11-7) | fail: audio | 2026-10-07 |
| `rastan-saga` | Rastan Saga | nesica | works (user) | pass | 2026-10-07 |
| `senko-no-ronde-duo` | Senko no Ronde DUO: Dis-United Order | nesica | works, sound effects (user) | pass | 2026-10-07 |
| `skullgirls-2nd-encore` | Skullgirls 2nd Encore | nesica | works (user) | pass | 2026-10-07 |
| `space-invaders` | Space Invaders | nesica | works (user) | fail: picture, audio | 2026-10-07 |
| `strania` | Strania: The Stella Machina | nesica | works (user), music and sound effects | pass | 2026-10-07 |
| `street-fighter-3-3rd-strike` | Street Fighter III 3rd Strike: Fight for the Future | nesica | works (user) | pass | 2026-10-07 |
| `street-fighter-zero-3` | Street Fighter Zero 3 | nesica | works fullscreen (user) | pass | 2026-10-07 |
| `the-rumble-fish-2` | The Rumble Fish 2 | nesica | works (user) | pass | 2026-10-07 |
| `tottemo-e-mahjong` | Tottemo E Mahjong | nesica | works (user); test menu (TestMode.exe) crashes, TODO | fail: picture, audio | 2026-10-07 |
| `trouble-witches-ac` | Trouble Witches AC: Amalgam no Joutachi | nesica | works (user) | pass | 2026-10-07 |
| `ultra-street-fighter-4` | Ultra Street Fighter 4 | nesica | works (user: 100%), opening video | pass | 2026-10-09 |
| `vampire-savior` | Vampire Savior: The Lord of Vampire | nesica | works (user) | pass | 2026-10-07 |
| `yatagarasu` | Yatagarasu: Attack on Cataclysm | nesica | works (user: 100%), fullscreen with its side portraits, Japanese text | pass | 2026-10-08 |
| `3d-cosplay-mahjong` | 3D Cosplay Mahjong | typex | in game (mahjong hand) | pass | 2026-10-07 |
| `battle-fantasia` | Battle Fantasia | typex | works (user: 100%) | pass | 2026-10-07 |
| `block-king-ball-shooter` | Block King Ball Shooter | typex | works (user: touch, coins, start, test menu; 4-player co-op as DemulShooter) | pass | 2026-10-07 |
| `blazblue-calamity-trigger` | BlazBlue Calamity Trigger | typex | in fight (user) | pass | 2026-10-07 |
| `chase-hq-2` | Chase H.Q. 2 | typex | works (user: 100%, wheel/pedals, switches, Nancy videos) | pass | 2026-10-09 |
| `gigawing-generations` | GigaWing Generations | typex | works (user), Landscape/Bezel dump rotated by its ReShade | pass | 2026-10-07 |
| `chaos-breaker-typex` | Chaos Breaker | typex | works (user: perfect) | pass | 2026-10-07 |
| `gaia-attack-4` | Gaia Attack 4 | typex | works (user: 100%, 4 guns, coins, sound, videos) | pass | 2026-10-09 |
| `gouketsuji-ichizoku-typex2` | Gouketsuji Ichizoku - Matsuri Senzo Kuyou | typex | works (user: 100%) | pass | 2026-10-09 |
| `kof-98-um-typex` | The King of Fighters '98 Ultimate Match | typex | works (user: perfect) | pass | 2026-10-07 |
| `kof-sky-stage` | The King of Fighters Sky Stage | typex | works (user), rotated by the dump's ReShade | pass | 2026-10-07 |
| `haunted-museum` | Haunted Museum | typex | works (user: 100%, guns, service/test, sound) | pass | 2026-10-07 |
| `gundam-spirits-of-zeon` | Mobile Suit Gundam: Spirits of Zeon | typex | works (user: guns, inputs, coins; intro video fixed with `WAL_D3D9_POW2`) | pass | 2026-10-07 |
| `gundam-spirits-of-zeon-2p` | Mobile Suit Gundam: Spirits of Zeon (2 players) | typex | works (user: perfect, in gamescope, guns) | fail: window | 2026-10-07 |
| `haunted-museum-2` | Haunted Museum II | typex | works (user: guns, inputs, sound, video) | pass | 2026-10-07 |
| `k-on-after-school-rhythm-selection` | K-On! After School Rhythm Selection | typex | BLOCKED: error 0002 DISPENSER_ERROR (card dispenser) | not in the test |  |
| `king-of-fighters-maximum-impact-regulation-a` | King of Fighters Maximum Impact Regulation A | typex | works (user: 100%), intro movie | pass | 2026-10-07 |
| `king-of-fighters-xii` | The King of Fighters XII | typex | in game, intro video | pass | 2026-10-07 |
| `king-of-fighters-xiii` | The King of Fighters XIII | typex | works (user: 100%) | pass | 2026-10-09 |
| `music-gungun-2` | Music GunGun! 2 | typex | works (user: perfect), attract movie, sound, 2 guns | pass | 2026-10-09 |
| `raiden-3-typex` | Raiden III | typex | works (user); intro movie black (as NESiCA) | pass | 2026-10-07 |
| `raiden-4-typex` | Raiden IV | typex | works (user) | pass | 2026-10-07 |
| `shikigami-no-shiro-3` | Shikigami no Shiro III | typex | works (user), Landscape/Bezel dump rotated by its ReShade | pass | 2026-10-07 |
| `senko-no-ronde-duo-typex2` | Senko no Ronde DUO: Dis-United Order | typex | works (user: perfect) | pass | 2026-10-07 |
| `spica-adventure` | Spica Adventure | typex | works (user: 100%) | pass | 2026-10-09 |
| `new-super-mario-bros-wii-coin-world` | New Super Mario Bros. Wii Coin World | typex | works (user: 100%), 4 satellites, medals, hoppers, satellite test menu | pass | 2026-10-09 |
| `taisen-hot-gimmick-5` | Taisen Hot Gimmick 5: Mirai Eigou | typex | works (user: 100%) | fail: picture, audio | 2026-10-09 |
| `tetris-the-grand-master-3` | Tetris The Grand Master 3 Terror-Instinct | typex | works (user: perfect) | pass | 2026-10-07 |
| `street-fighter-iv` | Street Fighter IV | typex | works (user: perfect), intro video plays | pass | 2026-10-07 |
| `valve-limit-r` | Valve Limit R | typex | works (user), wheel, pedals, races start at once; TODO: option to hide the passenger girl's cut-ins | not in the test |  |
| `revolt` | Re-Volt (Tsunami cabinet) | tsunami | works (user-confirmed): `-launchGame`, coin then the gas pedal starts a race; wheel, pedals and cabinet buttons through the TsuInput object (GetJoyInfo) | not in the test |  |
<!-- GAMELIST END -->
