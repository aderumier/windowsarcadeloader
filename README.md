# Windows Arcade Loader

Runs Windows-based arcade games (Taito Type X / X2, NESiCAxLive, Global VR, Namco ES3, Tsunami, Sega RingEdge, Sega Amusements Shell PCs, Wahlap, Sega Europa-R, Konami e-Amusement PC, UNIS, arcade PCs)
on Linux with Wine: a Linux launcher (inputs, Wine prefix and runner) and per-system payload DLLs
emulating each cabinet's I/O, dongle and network services in the game process.

* Build: `./build.sh` (`dist/arcade-launcher`, `dist/payloads/*.dll`)
* Run: `dist/arcade-launcher run <dump>/<gameid>.windowsloader`
* Frontend: `dist/arcade-launcher` without a game opens the GUI: every game, its controls mapped
  per device (keyboard, gamepad, wheel) into `userprofiles/`, its dump folder, launch. Esc (any
  keyboard) or Back+Start stops the game. "Wheel calibration" calibrates a wheel and its pedals
  once (`userprofiles/wheels.yaml`): it is then read as a gamepad with the gamepad's full ranges;
  each game's Wheel tab maps it, by default as its gamepad mapping.
  "Players" sets which gamepad is P1, P2...: a priority list (`userprofiles/players.yaml`, by
  SDL GUID) whose connected devices go first, in its order, over the frontend's order; a gamepad
  joins a wheel, always P1.
  "NESiCA card" sets the player's card of every NESiCA game (`userprofiles/nesica-card.yaml`):
  card id ("New card": a fresh one) and player name (games whose card has one: Fighting EX
  Layer); each game keeps its own progress on it.
  "e-Amusement card" sets the player's card of every Konami game
  (`userprofiles/eamuse-card.yaml`): card id and PIN (with a PIN, the loader's e-Amusement
  server registers the card: the game only asks the PIN); each game keeps its own progress.
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
| `mario-kart-arcade-gp-dx` | Mario Kart Arcade GP DX (Namco ES3) | namcoes3 | works (user: 100%); Namcam and Banapass card reader not implemented | not in the test |  |
| `point-blank-x` | Point Blank X | namcoes3 | works (headless tests: coins, start, aim and shots, operator menu), 2 guns | not in the test |  |
| `mach-storm` | Mach Storm | namcoes3 | works (user: 100%) | not in the test |  |
| `lost-land-adventure` | Lost Land Adventure | namcoes3 | works (user: 100%), 2 guns; test menu behind the game: alt-tab | not in the test |  |
| `farcry-paradise-lost` | Far Cry Paradise Lost | globalvr | not playable (user: in-game bug) | not in the test |  |
| `aliens-extermination` | Aliens Extermination | globalvr | works (user: 100%), 2 guns (trigger also starts), movies | pass | 2026-10-11 |
| `the-swarm` | The Swarm | globalvr | works (user: 100%), 2 guns (fire, grenade, shield); stage changes: the game started again with the next map (Relaunch message), works (user: 100%); new high scores not saved | pass | 2026-10-11 |
| `need-for-speed` | Need for Speed | globalvr | works (user: 100%) | not in the test |  |
| `need-for-speed-gt` | Need for Speed GT | globalvr | works (user: 100%) | not in the test |  |
| `need-for-speed-underground` | Need for Speed Underground | globalvr | works (user: 100%) | not in the test |  |
| `need-for-speed-carbon` | Need for Speed Carbon | globalvr | works (user: 100%); force feedback not tested on a wheel | not in the test |  |
| `nascar-racing` | NASCAR Racing | globalvr | works (user: 100%) | not in the test |  |
| `nascar-team-racing` | NASCAR Team Racing | globalvr | works (user: 100%) | not in the test |  |
| `blazing-angels` | Blazing Angels: Squadrons of WWII | globalvr | works (user: 100%) | not in the test |  |
| `beach-head-2000` | Beach Head 2000 | globalvr | works (user: 100%) | not in the test |  |
| `beach-head-2002` | Beach Head 2002 | globalvr | works (user: 100%) | not in the test |  |
| `beach-head-2003-desert-war` | Beach Head 2003 Desert War | globalvr | works (user: 100%), machine gun, cannon, missiles | not in the test |  |
| `twisted-nitro-stunt-racing` | Twisted: Nitro Stunt Racing | globalvr | works (user: 100%) | not in the test |  |
| `akai-katana-shin` | Akai Katana Shin | nesica | in game (GAME_START) | pass | 2026-10-07 |
| `aquapazza` | Aquapazza: Aquaplus Dream Match | nesica | works (user: 100%) | pass | 2026-10-08 |
| `arcana-heart-2` | Arcana Heart 2 | nesica | in game (user) | pass | 2026-10-07 |
| `arcana-heart-3-lmss` | Arcana Heart 3 Love Max Six Stars!!!!!! | nesica | in game (user, GAME_START) | pass | 2026-10-07 |
| `suggoi-arcana-heart-2` | Suggoi! Arcana Heart 2 | nesica | works (user: 100%) | pass | 2026-10-17 |
| `battle-fantasia-nesica` | Battle Fantasia (NESiCAxLive) | nesica | works (user: 100%) | not in the test |  |
| `spica-adventure-nesica` | Spica Adventure (NESiCAxLive) | nesica | works (user: 100%) | not in the test |  |
| `blazblue-central-fiction` | BlazBlue Central Fiction 2.01 | nesica | in game, NESiCA online | pass | 2026-10-07 |
| `blazblue-chronophantasma` | BlazBlue Chronophantasma 2.03 | nesica | works (user: 100%) | pass | 2026-10-09 |
| `blazblue-continuum-shift-2` | BlazBlue Continuum Shift II | nesica | works (user: 100%) | pass | 2026-10-11 |
| `chaos-breaker` | Chaos Breaker | nesica | in fight, music | pass | 2026-10-07 |
| `chaos-code-103` | Chaos Code: New Sign of Catastrophe 1.03 | nesica | in fight (user) | pass | 2026-10-07 |
| `chaos-code-211` | Chaos Code: New Sign of Catastrophe 2.11 | nesica | in fight (user) | pass | 2026-10-07 |
| `crimzon-clover` | Crimzon Clover | nesica | works fullscreen (user; 1280x800: pillarboxed on a 16:9 screen) | fail: picture | 2026-10-07 |
| `daemon-bride` | Daemon Bride: Additional Gain | nesica | in fight | pass | 2026-10-07 |
| `dariusburst-another-chronicle` | Dariusburst Another Chronicle (1.05) | nesica | works (user: 100%) | fail: window | 2026-10-19 |
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
| `million-arthur-arcana-blood` | Million Arthur Arcana Blood | nesica2 | works (user: 100%: training and story modes), NESiCA network connected | not in the test |  |
| `kof-xiv` | The King of Fighters XIV | nesica2 | works (user: 100%), NESiCA network connected | not in the test |  |
| `snk-heroines` | SNK Heroines Tag Team Frenzy | nesica2 | works (user: 100%), NESiCA network connected | not in the test |  |
| `samurai-shodown-130` | Samurai Shodown (1.30.00) | nesica2 | works (user: 100%), NESYS offline; a red banner ただいまネットワークを使ったプレイができません。 (network play unavailable) stays on screen: the game's own offline notice | not in the test |  |
| `samurai-shodown-180` | Samurai Shodown (1.80.00) | nesica2 | works (user: 100%), NESYS offline; a red banner ただいまネットワークを使ったプレイができません。 (network play unavailable) stays on screen: the game's own offline notice | not in the test |  |
| `fighting-ex-layer` | Fighting EX Layer | nesica2 | in game (user: card accepted, game starts), NESiCA network connected; card saved (user: name shown, no freeze after a match) | not in the test |  |
| `theatrhythm-final-fantasy-all-star-carnival` | Theatrhythm Final Fantasy All-Star Carnival | nesica2 | works (user: 100%), NESiCA cards (loader's card server, progress saved), touch screen (mouse, red pointer); right lever on I/J/K/L; no matching with other cabinets | not in the test |  |
| `3d-cosplay-mahjong` | 3D Cosplay Mahjong | typex | in game (mahjong hand) | pass | 2026-10-07 |
| `battle-fantasia` | Battle Fantasia | typex | works (user: 100%) | pass | 2026-10-10 |
| `cyber-diver` | Cyber Diver | typex | works (user: 100%) | fail: window | 2026-10-19 |
| `sonic-blast-heroes` | Sonic Blast Heroes | typex | works (user: 100%) | not in the test |  |
| `block-king-ball-shooter` | Block King Ball Shooter | typex | works (user: touch, coins, start, test menu; 4-player co-op as DemulShooter) | pass | 2026-10-07 |
| `blazblue-calamity-trigger` | BlazBlue Calamity Trigger | typex | in fight (user) | pass | 2026-10-07 |
| `blazblue-continuum-shift` | BlazBlue Continuum Shift | typex | works (user: 100%) | pass | 2026-10-10 |
| `chase-hq-2` | Chase H.Q. 2 | typex | works (user: 100%, wheel/pedals, switches, Nancy videos) | pass | 2026-10-10 |
| `gigawing-generations` | GigaWing Generations | typex | works (user), Landscape/Bezel dump rotated by its ReShade | pass | 2026-10-07 |
| `chaos-breaker-typex` | Chaos Breaker | typex | works (user: perfect) | fail: running, window | 2026-10-10 |
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
| `elevator-action-death-parade` | Elevator Action Death Parade | typex | works (user: 100%, perfect), 2 guns, attraction doors and bezel, crosshairs option | pass | 2026-10-11 |
| `king-of-fighters-xiii` | The King of Fighters XIII | typex | works (user: 100%) | pass | 2026-10-09 |
| `kof-xiii-climax-typex2` | The King of Fighters XIII Climax (Type X2) | typex | works (user: 100%) | pass | 2026-10-10 |
| `music-gungun-2` | Music GunGun! 2 | typex | works (user: perfect), attract movie, sound, 2 guns | pass | 2026-10-09 |
| `raiden-3-typex` | Raiden III | typex | works (user); intro movie black (as NESiCA) | pass | 2026-10-07 |
| `raiden-4-typex` | Raiden IV | typex | works (user) | pass | 2026-10-07 |
| `shikigami-no-shiro-3` | Shikigami no Shiro III | typex | works (user), Landscape/Bezel dump rotated by its ReShade | pass | 2026-10-07 |
| `samurai-spirits-sen` | Samurai Spirits Sen | typex | works (user: 100%) | pass | 2026-10-10 |
| `senko-no-ronde-duo-typex2` | Senko no Ronde DUO: Dis-United Order | typex | works (user: perfect) | pass | 2026-10-07 |
| `spica-adventure` | Spica Adventure | typex | works (user: 100%) | pass | 2026-10-09 |
| `new-super-mario-bros-wii-coin-world` | New Super Mario Bros. Wii Coin World | typex | works (user: 100%), 4 satellites, medals, hoppers, satellite test menu | pass | 2026-10-09 |
| `taisen-hot-gimmick-5` | Taisen Hot Gimmick 5: Mirai Eigou | typex | works (user: 100%) | fail: picture, audio | 2026-10-09 |
| `tetris-the-grand-master-3` | Tetris The Grand Master 3 Terror-Instinct | typex | works (user: perfect) | pass | 2026-10-07 |
| `street-fighter-iv` | Street Fighter IV | typex | works (user: perfect), intro video plays | pass | 2026-10-07 |
| `super-street-fighter-iv-ae` | Super Street Fighter IV Arcade Edition (export, standalone) | typex | works (user: 100%), intro video plays | pass | 2026-10-11 |
| `super-street-fighter-iv-ae-cabinet` | Super Street Fighter IV Arcade Edition | typex | works (user: 100%) | not in the test |  |
| `super-street-fighter-iv-ae-2012` | Super Street Fighter IV Arcade Edition Ver. 2012 | typex | works (user: 100%) | not in the test |  |
| `valve-limit-r` | Valve Limit R | typex | works (user), wheel, pedals, races start at once; TODO: option to hide the passenger girl's cut-ins | not in the test |  |
| `d1gp-arcade` | D1GP Arcade | typex | works (user: wheel, pedals, buttons, test menu); TODO: force feedback check on a wheel, magnetic card reader (saves) | not in the test |  |
| `wacky-races` | Wacky Races | typex | works (user: controls, start, view) | not in the test |  |
| `battle-gear-4` | Battle Gear 4 | typex | works (user: 100%) | not in the test |  |
| `battle-gear-4-tuned` | Battle Gear 4 Tuned | typex | works (user: wheel, pedals, buttons, intro, music, coins), test menu | pass | 2026-10-10 |
| `battle-gear-4-tuned-pro` | Battle Gear 4 Tuned (professional) | typex | works (user: H shifter, clutch, coins in the I/O test) | pass | 2026-10-10 |
| `fast-and-furious` | The Fast and the Furious | rawthrills | works (user: 100%) | not in the test |  |
| `fast-and-furious-drift` | The Fast and the Furious Drift | rawthrills | works (user: 100%) | not in the test |  |
| `fast-and-furious-supercars` | The Fast and the Furious SuperCars | rawthrills | works (user: 100%) | not in the test |  |
| `fast-and-furious-super-bikes` | The Fast and the Furious Super Bikes | rawthrills | works (user: 100%) | not in the test |  |
| `fast-and-furious-super-bikes-2` | The Fast and the Furious Super Bikes 2 | rawthrills | works (user: 100%) | not in the test |  |
| `dirty-drivin` | Dirty Drivin' | rawthrills | works (user: 100%) | not in the test |  |
| `frogger` | Frogger (2011 redemption) | rawthrills | works (user: 100%), two-player controls, service menu, volume | not in the test |  |
| `cars` | Cars | rawthrills | in game (user: accel, TEST, calibration) | not in the test |  |
| `nicktoons-nitro-racing` | Nicktoons Nitro Racing | rawthrills | works (user: 100%) | not in the test |  |
| `batman` | Batman | rawthrills | works (user-confirmed gameplay and controls; FFB wheel test pending) | not in the test |  |
| `injustice-arcade` | Injustice Arcade | rawthrills | works (user: 100%, virtual card readers) | not in the test |  |
| `teenage-mutant-ninja-turtles` | Teenage Mutant Ninja Turtles | rawthrills | works (user: 100%), four-player controls, test menu, variable-height jump, videos | not in the test |  |
| `super-alpine-racer` | Super Alpine Racer | rawthrills | in progress: boots, a race played from scripted inputs (swing, start) | not in the test |  |
| `x-games-snowboarder` | X-Games Snowboarder | rawthrills | in progress: boots, a race played from scripted inputs | not in the test |  |
| `winter-x-games-snocross` | Winter X Games SnoCross | rawthrills | works (user: 100%) | not in the test |  |
| `h2o-overdrive` | H2O Overdrive | rawthrills | works (user: 100%) | not in the test |  |
| `guitar-hero-arcade` | Guitar Hero Arcade EDITION | rawthrills | works (user: 100%) | not in the test |  |
| `virtua-tennis-4` | Virtua Tennis 4 | ringedge | works (user: 100%), test menu | not in the test |  |
| `guilty-gear-xx-accent-core-plus-r` | Guilty Gear XX Accent Core Plus R | ringedge | works (user: 100%) | not in the test |  |
| `under-night-in-birth-exe-late` | Under Night In-Birth Exe:Late | ringedge | works (user: 100%), Aime offline banner removed; no Aime card play | not in the test |  |
| `under-night-in-birth-exe-late-st` | Under Night In-Birth Exe:Late[st] | ringedge | works (user: 100%), Aime offline banner removed; no Aime card play | not in the test |  |
| `koihime-enbu` | Koihime Enbu | ringedge | works (user: 100%) | not in the test |  |
| `blade-arcus-from-shining` | Blade Arcus from Shining (2.10.00) | ringedge | works (user: 100%), Aime offline banner removed; no Aime card play | not in the test |  |
| `blade-strangers` | Blade Strangers (1.02.00, 1.3.2) | ringedge | works (user: 100%) | not in the test |  |
| `caladrius-ac` | Caladrius AC | ringedge | works (user: 100%) | not in the test |  |
| `melty-blood-actress-again-current-code-re2` | Melty Blood Actress Again Current Code (RE2) | ringedge | works (user: 100%) | not in the test |  |
| `phantom-breaker-another-code` | Phantom Breaker: Another Code | ringedge | works (user: 100%) | not in the test |  |
| `ge-sen-love-plus-pengo` | Ge-Sen Love Plus Pengo! | ringedge | works (user: 100%) | not in the test |  |
| `transformers-human-alliance` | Transformers: Human Alliance | ringedge | works (user: 100%) | not in the test |  |
| `guilty-gear-xrd-rev2` | Guilty Gear Xrd Rev 2 | ringedge | works (user: 100%) | not in the test |  |
| `guilty-gear-xrd-sign` | Guilty Gear Xrd -SIGN- | ringedge | works (user: 100%) | not in the test |  |
| `guilty-gear-xrd-revelator` | Guilty Gear Xrd -REVELATOR- | ringedge | works (user: 100%) | not in the test |  |
| `dengeki-bunko-fighting-climax` | Dengeki Bunko: Fighting Climax | ringedge | works (user: 100%) | not in the test |  |
| `dengeki-bunko-fighting-climax-ignition` | Dengeki Bunko: Fighting Climax Ignition | ringedge | works (user: 100%) | not in the test |  |
| `dead-or-alive-5-ultimate-arcade` | Dead or Alive 5 Ultimate Arcade | ringedge | works (user: 100%) | not in the test |  |
| `under-defeat-hd-plus` | Under Defeat HD+ | ringedge | works (user: 100%) | not in the test |  |
| `lets-go-island-3d` | Let's Go Island 3D: Lost on the Island of Tropics | ringedge | works (user: 100%) | not in the test |  |
| `lets-go-island-ringwide` | Let's Go Island: Lost on the Island of Tropics (RingWide) | ringedge | works (user: 100%) | not in the test |  |
| `sega-golden-gun` | SEGA Golden Gun | ringedge | works (user: 100%) | not in the test |  |
| `dream-raiders` | Dream Raiders | ringedge | works (user: 100%) | not in the test |  |
| `operation-ghost` | Operation G.H.O.S.T. | ringedge | works (user: 100%) | not in the test |  |
| `target-bravo-operation-ghost` | Target Bravo: Operation G.H.O.S.T. | ringedge | works (user: 100%) | not in the test |  |
| `medal-no-gunman` | Medal no Gunman | ringedge | works (user: mouse) | not in the test |  |
| `super-monkey-ball-ticket-blitz` | Super Monkey Ball Ticket Blitz | ringedge | works (user: 100%) | not in the test |  |
| `lets-go-safari` | Let's Go Safari | ringedge | works (user: 100%) | not in the test |  |
| `lady-luck` | Lady Luck | ringedge | works (user: 100%) | not in the test |  |
| `teratoma` | Teratoma: The Last Rebellion | ringedge | works (user: 100%) | not in the test |  |
| `transformers-shadows-rising` | Transformers: Shadows Rising | segashell | works (user: 100%), test menu (user) | not in the test |  |
| `revolt` | Re-Volt (Tsunami cabinet) | tsunami | works (user-confirmed): `-launchGame`, coin then the gas pedal starts a race; wheel, pedals and cabinet buttons through the TsuInput object (GetJoyInfo) | not in the test |  |
| `storm-racer-g` | Storm Racer G | wahlap | works (user: 100%) | not in the test |  |
| `storm-rider` | Storm Rider | wahlap | works (user: 100%) | not in the test |  |
| `ford-racing-full-blown` | Ford Racing Full Blown | europar | works (user: 100%) | not in the test |  |
| `sega-rally-3` | SEGA Rally 3 | europar | works (user: 100%) | not in the test |  |
| `showdown` | Showdown | segashell | works (user: 100%) | not in the test |  |
| `grid` | GRID | europar | works (user: 100%) | not in the test |  |
| `nobiteke-byon-byon` | Nobiteke! Byon Byon Daisakusen | konami | works (user: 100%) | not in the test |  |
| `gashaaaan` | Gashaaaan | konami | works (headless test) | not in the test |  |
| `gashaaaan-refill` | Gashaaaan Refill! | konami | works (user: 100%) | not in the test |  |
| `otomedius` | Otomedius | konami | works (user: 100%) | not in the test |  |
| `winning-eleven-2008` | Winning Eleven 2008 | konami | works (user: 100%), e-Amusement cards (loader's server) | not in the test |  |
| `gti-club-supermini-festa` | GTI Club: Supermini Festa! | konami | works (user: 100%) | not in the test |  |
| `winning-eleven-2012` | Winning Eleven 2012 | konami | works (user: 100%); e-Amusement cards (login, new player, match; save at a game's end not verified yet) | not in the test |  |
| `bishi-bashi-channel` | Bishi Bashi Channel | konami | works (headless: attract, a 1P game) | not in the test |  |
| `the-bishi-bashi` | The Bishi Bashi | konami | works (user: 100%) | not in the test |  |
| `dance-dance-revolution-2013` | Dance Dance Revolution 2013 | konami | works (user: 100%), e-Amusement cards (loader's server) | not in the test |  |
| `yugioh-duel-terminal-6` | Yu-Gi-Oh! 5D's Duel Terminal 6 | konami | works (user: 100%) | not in the test |  |
| `hello-popn-music` | Hello! Pop'n Music | konami | works (user: 100%), e-Amusement cards for both players (loader's server) | not in the test |  |
| `metal-gear-arcade` | Metal Gear Arcade | konami | works (user: 100%), gun, e-Amusement cards (loader's server), test menu | not in the test |  |
| `steel-chronicle-victroopers` | Steel Chronicle Victroopers | konami | works (user: 100%), two flight sticks, touch screen (mouse or gun), e-Amusement cards (loader's server) | not in the test |  |
| `silent-scope-bone-eater` | Silent Scope: Bone Eater (Widescreen Edition) | konami | works (user: 100%), gun or mouse, scope tap / hold | not in the test |  |
| `road-fighters-3d` | Road Fighters 3D | konami | works (headless tests: attract, race, test menu) | not in the test |  |
| `contra-evolution` | Contra: Evolution | konami | works (user: 100%) | not in the test |  |
| `wartran-troopers` | Wartran Troopers | konami | works (user: 100%) | not in the test |  |
| `lethal-enforcers-3` | Lethal Enforcers 3 | konami | works (user: 100%), 1 gun, player 1's screen (rival's screen option), test menu | not in the test |  |
| `lethal-enforcers-3-2p` | Lethal Enforcers 3 (2 players) | konami | works (user: 100%), 2 guns, 2 screens side by side, test menu | not in the test |  |
| `coopers-9` | Cooper's 9 | konami | works (user: 100%), 2 guns, test menu | not in the test |  |
| `rainbow-bombergirl` | Rainbow Bomber Girl | konami | works (user: 100%) | not in the test |  |
| `galaxy-rangers` | Galaxy Rangers | arcadepc | works (headless tests: coins, start, level select, aim and shots, settings menu), 4 guns | not in the test |  |
| `quick-shot-arcade` | Quick Shot Arcade | arcadepc | user 100% (2026-10-26: display, crosshair, logo, bezel), 1 gun | not in the test |  |
| `top-gun-arcade` | Top Gun Arcade | arcadepc | user 100% (2026-10-26: display, crosshair, settings menu, bezel), 1 gun | not in the test |  |
| `happy-water-sprite-2` | Happy Water Sprite II | arcadepc | user 100% (2026-10-26), 2 water guns | not in the test |  |
| `mech-tornado` | Mech Tornado | arcadepc | user 100% (2026-10-26: laser mode, 4 players), 4 guns | not in the test |  |
| `fire-hero` | Fire Hero | arcadepc | user 100% (2026-10-26), 2 water cannons | not in the test |  |
| `zombie-crisis` | Zombie Crisis | arcadepc | user 100% (2026-10-26), 2 guns | not in the test |  |
| `bullseye-crack-shot` | Bullseye Crack Shot | arcadepc | user 100%, 1 gun | not in the test |  |
| `hot-shots-fire-rescue` | Hot Shots Fire Rescue | arcadepc | user 100% (2026-10-26), 4 water cannons | not in the test |  |
| `skull-of-shadow` | Skull of Shadow | arcadepc | user 100% (2026-10-26), 4 guns | not in the test |  |
| `dino-invasion` | Dino Invasion | unis | user 100% (2026-10-26), 4 guns | not in the test |  |
| `mech-dinosaur` | Mech Dinosaur | arcadepc | user 100% (2026-10-26, all maps open), 4 guns | not in the test |  |
| `wisdom-zombies` | Wisdom Zombies | arcadepc | user 100% (2026-10-26, all maps open), 4 water guns | not in the test |  |
<!-- GAMELIST END -->
