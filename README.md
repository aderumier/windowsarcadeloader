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

## Games (402)

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
| `time-crisis-5` | Time Crisis 5 | namcoes3 | works (headless tests: coins, pedals, aim and shots, tutorial, operator menu), 1 gun | not in the test |  |
| `tekken-7` | Tekken 7 | namcoes3 | works (user: 100%, online with Banapass cards), 1 player per cabinet | not in the test |  |
| `tekken-7-fated-retribution` | Tekken 7 Fated Retribution | namcoes3 | works (user: 100%, online with Banapass cards), 1 player per cabinet | not in the test |  |
| `tekken-7-fated-retribution-round-2` | Tekken 7 Fated Retribution Round 2 | namcoes3 | works (user: 100%, online with Banapass cards, insignia rate boost), 1 player per cabinet | not in the test |  |
| `pokken-tournament` | Pokkén Tournament | namcoes3 | user 100% (24.05 with Banapassport cards, 26.02 offline), 1 player per cabinet, English text | not in the test |  |
| `jojos-bizarre-adventure-last-survivor` | JoJo's Bizarre Adventure - Last Survivor | namcoes3 | works offline (headless tests: system check, coins, start, PP purchase, mode select, operator menu), touch with the mouse, English text; no game server: national / store battles off | not in the test |  |
| `star-wars-battle-pod` | Star Wars Battle Pod - The Force Awakens | namcoes3 | boots (2026-10-06), not user-verified | not in the test |  |
| `wangan-midnight-maximum-tune-5` | Wangan Midnight Maximum Tune 5 | namcoes3 | works (user: game works; headless: cards, new player, car, race, save) | not in the test |  |
| `wangan-midnight-maximum-tune-5dx` | Wangan Midnight Maximum Tune 5DX | namcoes3 | works (user), full screen; cards and game server | not in the test |  |
| `wangan-midnight-maximum-tune-5dxp` | Wangan Midnight Maximum Tune 5DX+ | namcoes3 | works (user), English; cards and game server | not in the test |  |
| `wangan-midnight-maximum-tune-6` | Wangan Midnight Maximum Tune 6 | namcoes3 | works (user: 100%), cards and game server | not in the test |  |
| `wangan-midnight-maximum-tune-6r` | Wangan Midnight Maximum Tune 6R | namcoes3 | works (user: 100%) | not in the test |  |
| `wangan-midnight-maximum-tune-6rr` | Wangan Midnight Maximum Tune 6RR | namcoes3 | works (user: 100%) | not in the test |  |
| `farcry-paradise-lost` | Far Cry Paradise Lost | globalvr | works (user: 100%) | not in the test |  |
| `aliens-extermination` | Aliens Extermination | globalvr | works (user: 100%), 2 guns (trigger also starts), movies | not in the test |  |
| `the-swarm` | The Swarm | globalvr | works (user: 100%), 2 guns (fire, grenade, shield); stage changes: the game started again with the next map (Relaunch message), works (user: 100%); new high scores not saved | not in the test |  |
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
| `redline-rampage` | Redline Rampage | globalvr | works (user: 100%) | not in the test |  |
| `justice-league-heroes-united` | Justice League: Heroes United | globalvr | works (user: 100%), 2 players | not in the test |  |
| `madden-nfl-season-2` | EA SPORTS Madden NFL Football Season 2 | globalvr | works (user: 100%) | not in the test |  |
| `sin` | SiN | globalvr | works (user: 100%) | not in the test |  |
| `heavy-gear-ii` | Heavy Gear II | globalvr | works (user: 100%) | not in the test |  |
| `pga-tour-golf-championship-edition-iii` | PGA TOUR Golf Championship Edition III | globalvr | works (user: 100%) | not in the test |  |
| `pga-tour-golf-championship-edition-ii` | PGA TOUR Golf Championship Edition II | globalvr | works (user: 100%) | not in the test |  |
| `pga-tour-golf` | PGA TOUR Golf | globalvr | works (user: 100%), trackball: the mouse or touchpad (or stick) | not in the test |  |
| `pga-tour-golf-team-challenge` | PGA TOUR Golf Team Challenge | globalvr | works (user: 100%), trackball: the mouse or touchpad (or stick) | not in the test |  |
| `ultrapin` | UltraPin | globalvr | works (user: 100%) | not in the test |  |
| `akai-katana-shin` | Akai Katana Shin | nesica | in game (GAME_START) | not in the test |  |
| `aquapazza` | Aquapazza: Aquaplus Dream Match | nesica | works (user: 100%) | not in the test |  |
| `arcana-heart-2` | Arcana Heart 2 | nesica | in game (user) | not in the test |  |
| `arcana-heart-3-lmss` | Arcana Heart 3 Love Max Six Stars!!!!!! | nesica | in game (user, GAME_START) | not in the test |  |
| `suggoi-arcana-heart-2` | Suggoi! Arcana Heart 2 | nesica | works (user: 100%) | not in the test |  |
| `battle-fantasia-nesica` | Battle Fantasia (NESiCAxLive) | nesica | works (user: 100%) | not in the test |  |
| `spica-adventure-nesica` | Spica Adventure (NESiCAxLive) | nesica | works (user: 100%) | not in the test |  |
| `blazblue-central-fiction` | BlazBlue Central Fiction 2.01 | nesica | in game, NESiCA online | not in the test |  |
| `blazblue-chronophantasma` | BlazBlue Chronophantasma 2.03 | nesica | works (user: 100%) | not in the test |  |
| `blazblue-continuum-shift-2` | BlazBlue Continuum Shift II | nesica | works (user: 100%) | not in the test |  |
| `chaos-breaker` | Chaos Breaker | nesica | in fight, music | not in the test |  |
| `chaos-code-103` | Chaos Code: New Sign of Catastrophe 1.03 | nesica | in fight (user) | not in the test |  |
| `chaos-code-211` | Chaos Code: New Sign of Catastrophe 2.11 | nesica | in fight (user) | not in the test |  |
| `crimzon-clover` | Crimzon Clover | nesica | works fullscreen (user: 100%; 1280x800: pillarboxed on a 16:9 screen) | not in the test |  |
| `daemon-bride` | Daemon Bride: Additional Gain | nesica | in fight | not in the test |  |
| `dariusburst-another-chronicle` | Dariusburst Another Chronicle (1.05) | nesica | works (user: 100%) | not in the test |  |
| `dariusburst-another-chronicle-ex` | Dariusburst Another Chronicle EX | nesica | works (user: 100%), 4 players, sound, test menu in Japanese | not in the test |  |
| `dark-awake` | Dark Awake: The King Has No Name | nesica | in fight | not in the test |  |
| `do-not-fall` | Do Not Fall: Run for Your Drink | nesica | works (user) | not in the test |  |
| `dragon-dance` | Dragon Dance | nesica | works (user), smoke and sparkle effects correct with d7vk; crash on applying saved display settings fixed by a code patch | not in the test |  |
| `elevator-action` | Elevator Action Death Parade | nesica | works (user: 100%), 4:3 | not in the test |  |
| `en-eins-perfektewelt` | EN-Eins Perfektewelt | nesica | works (user) | not in the test |  |
| `exception` | Exception | nesica | works fullscreen (user) | not in the test |  |
| `gouketsuji-ichizoku` | Gouketsuji Ichizoku: Matsuri Senzo Kuyou | nesica | works (user) | not in the test |  |
| `homura` | Homura | nesica | works (user), sound effects and music in game; attract demo silent, as under Windows (original or dump, not the loader) | not in the test |  |
| `hyper-street-fighter-2` | Hyper Street Fighter II: The Anniversary Edition | nesica | works (user) | not in the test |  |
| `ikaruga` | Ikaruga | nesica | in game (user) | not in the test |  |
| `kof-2002-um` | The King of Fighters 2002 Unlimited Match | nesica | works (user) | not in the test |  |
| `kof-98-umfe` | The King of Fighters '98 Ultimate Match Final Edition | nesica | works (user) | not in the test |  |
| `kof-xiii-climax` | The King of Fighters XIII Climax | nesica | in game, movies | not in the test |  |
| `magical-beat` | Magical Beat | nesica | works (user) | not in the test |  |
| `nitroplus-blasterz` | Nitroplus Blasterz: Heroines Infinite Duel | nesica | works (user) | not in the test |  |
| `persona-4-arena` | Persona 4 The Ultimate in Mayonaka Arena | nesica | works (user: perfect) | not in the test |  |
| `persona-4-ultimax` | Persona 4 The Ultimax Ultra Suplex Hold | nesica | works (user) | not in the test |  |
| `psychic-force-2012` | Psychic Force 2012 | nesica | works (user) | not in the test |  |
| `puzzle-bobble` | Puzzle Bobble | nesica | works (user) | not in the test |  |
| `raiden-3` | Raiden III | nesica | in game (user); intro movie black | not in the test |  |
| `raiden-4` | Raiden IV | nesica | in game (user), intro movie (with GE-Proton's winedmo MPEG sequence header fix, after 11-7) | not in the test |  |
| `rastan-saga` | Rastan Saga | nesica | works (user) | not in the test |  |
| `senko-no-ronde-duo` | Senko no Ronde DUO: Dis-United Order | nesica | works, sound effects (user) | not in the test |  |
| `skullgirls-2nd-encore` | Skullgirls 2nd Encore | nesica | works (user) | not in the test |  |
| `space-invaders` | Space Invaders | nesica | works (user: 100%) | not in the test |  |
| `strania` | Strania: The Stella Machina | nesica | works (user), music and sound effects | not in the test |  |
| `street-fighter-3-3rd-strike` | Street Fighter III 3rd Strike: Fight for the Future | nesica | works (user) | not in the test |  |
| `street-fighter-zero-3` | Street Fighter Zero 3 | nesica | works fullscreen (user) | not in the test |  |
| `the-rumble-fish-2` | The Rumble Fish 2 | nesica | works (user) | not in the test |  |
| `tottemo-e-mahjong` | Tottemo E Mahjong | nesica | works (user); test menu (TestMode.exe) crashes, TODO | not in the test |  |
| `trouble-witches-ac` | Trouble Witches AC: Amalgam no Joutachi | nesica | works (user) | not in the test |  |
| `ultra-street-fighter-4` | Ultra Street Fighter 4 | nesica | works (user: 100%), opening video | not in the test |  |
| `vampire-savior` | Vampire Savior: The Lord of Vampire | nesica | works (user) | not in the test |  |
| `yatagarasu` | Yatagarasu: Attack on Cataclysm | nesica | works (user: 100%), fullscreen with its side portraits, Japanese text | not in the test |  |
| `magicians-dead` | Magician's Dead | nesica2 | works (user: 100%), NESiCA cards (new card registration, the loader's game server: profile and plays saved), tutorial and missions with computer players; Leap Motion hand from the right stick, the mouse or the touchpad | not in the test |  |
| `street-fighter-v` | Street Fighter V Type Arcade | nesica2 | works (user: 100%), NESiCA card (registration, play records saved), free play, arcade against the computer (one player per cabinet) | not in the test |  |
| `million-arthur-arcana-blood` | Million Arthur Arcana Blood | nesica2 | works (user: 100%: training and story modes), NESiCA network connected | not in the test |  |
| `kof-xiv` | The King of Fighters XIV | nesica2 | works (user: 100%), NESiCA network connected | not in the test |  |
| `snk-heroines` | SNK Heroines Tag Team Frenzy | nesica2 | works (user: 100%), NESiCA network connected | not in the test |  |
| `samurai-shodown-130` | Samurai Shodown (1.30.00) | nesica2 | works (user: 100%), NESYS offline; a red banner ただいまネットワークを使ったプレイができません。 (network play unavailable) stays on screen: the game's own offline notice | not in the test |  |
| `samurai-shodown-180` | Samurai Shodown (1.80.00) | nesica2 | works (user: 100%), NESYS offline; a red banner ただいまネットワークを使ったプレイができません。 (network play unavailable) stays on screen: the game's own offline notice | not in the test |  |
| `fighting-ex-layer` | Fighting EX Layer | nesica2 | in game (user: card accepted, game starts), NESiCA network connected; card saved (user: name shown, no freeze after a match) | not in the test |  |
| `theatrhythm-final-fantasy-all-star-carnival` | Theatrhythm Final Fantasy All-Star Carnival | nesica2 | works (user: 100%), NESiCA cards (loader's card server, progress saved), touch screen (mouse, red pointer); right lever on I/J/K/L; no matching with other cabinets | not in the test |  |
| `tetote-x-connect` | Tetote X Connect | nesica2 | works (user: 100%), NESiCA cards (loader's title server, progress saved), touch screen (mouse, touchpad multi-touch); no camera (pose phases play without it) | not in the test |  |
| `3d-cosplay-mahjong` | 3D Cosplay Mahjong | typex | in game (mahjong hand) | not in the test |  |
| `battle-fantasia` | Battle Fantasia | typex | works (user: 100%) | not in the test |  |
| `cyber-diver` | Cyber Diver | typex | works (user: 100%) | not in the test |  |
| `sonic-blast-heroes` | Sonic Blast Heroes | typex | works (user: 100%) | not in the test |  |
| `block-king-ball-shooter` | Block King Ball Shooter | typex | works (user: touch, coins, start, test menu; 4-player co-op as DemulShooter) | not in the test |  |
| `blazblue-calamity-trigger` | BlazBlue Calamity Trigger | typex | in fight (user) | not in the test |  |
| `blazblue-continuum-shift` | BlazBlue Continuum Shift | typex | works (user: 100%) | not in the test |  |
| `chase-hq-2` | Chase H.Q. 2 | typex | works (user: 100%, wheel/pedals, switches, Nancy videos) | not in the test |  |
| `gigawing-generations` | GigaWing Generations | typex | works (user), Landscape/Bezel dump rotated by its ReShade | not in the test |  |
| `chaos-breaker-typex` | Chaos Breaker | typex | works (user: perfect) | not in the test |  |
| `gaia-attack-4` | Gaia Attack 4 | typex | works (user: 100%, 4 guns, coins, sound, videos) | not in the test |  |
| `gouketsuji-ichizoku-typex2` | Gouketsuji Ichizoku - Matsuri Senzo Kuyou | typex | works (user: 100%) | not in the test |  |
| `kof-98-um-typex` | The King of Fighters '98 Ultimate Match | typex | works (user: perfect) | not in the test |  |
| `kof-sky-stage` | The King of Fighters Sky Stage | typex | works (user), rotated by the dump's ReShade | not in the test |  |
| `haunted-museum` | Haunted Museum | typex | works (user: 100%, guns, service/test, sound) | not in the test |  |
| `gundam-spirits-of-zeon` | Mobile Suit Gundam: Spirits of Zeon | typex | works (user: guns, inputs, coins; intro video fixed with `WAL_D3D9_POW2`) | not in the test |  |
| `gundam-spirits-of-zeon-2p` | Mobile Suit Gundam: Spirits of Zeon (2 players) | typex | works (user: perfect, in gamescope, guns) | not in the test |  |
| `haunted-museum-2` | Haunted Museum II | typex | works (user: guns, inputs, sound, video) | not in the test |  |
| `k-on-after-school-rhythm-selection` | K-On! After School Rhythm Selection | typex | works (headless tests: boot, card purchase, 3 virtual cards scanned, song select, rhythm game) | not in the test |  |
| `king-of-fighters-maximum-impact-regulation-a` | King of Fighters Maximum Impact Regulation A | typex | works (user: 100%), intro movie | not in the test |  |
| `king-of-fighters-xii` | The King of Fighters XII | typex | in game, intro video | not in the test |  |
| `elevator-action-death-parade` | Elevator Action Death Parade | typex | works (user: 100%, perfect), 2 guns, attraction doors and bezel, crosshairs option | not in the test |  |
| `king-of-fighters-xiii` | The King of Fighters XIII | typex | works (user: 100%) | not in the test |  |
| `kof-xiii-climax-typex2` | The King of Fighters XIII Climax (Type X2) | typex | works (user: 100%) | not in the test |  |
| `music-gungun-2` | Music GunGun! 2 | typex | works (user: perfect), attract movie, sound, 2 guns | not in the test |  |
| `raiden-3-typex` | Raiden III | typex | works (user); intro movie black (as NESiCA) | not in the test |  |
| `raiden-4-typex` | Raiden IV | typex | works (user) | not in the test |  |
| `shikigami-no-shiro-3` | Shikigami no Shiro III | typex | works (user), Landscape/Bezel dump rotated by its ReShade | not in the test |  |
| `samurai-spirits-sen` | Samurai Spirits Sen | typex | works (user: 100%) | not in the test |  |
| `senko-no-ronde-duo-typex2` | Senko no Ronde DUO: Dis-United Order | typex | works (user: perfect) | not in the test |  |
| `spica-adventure` | Spica Adventure | typex | works (user: 100%) | not in the test |  |
| `new-super-mario-bros-wii-coin-world` | New Super Mario Bros. Wii Coin World | typex | works (user: 100%), 4 satellites, medals, hoppers, satellite test menu | not in the test |  |
| `taisen-hot-gimmick-5` | Taisen Hot Gimmick 5: Mirai Eigou | typex | works (user: 100%) | not in the test |  |
| `usagi-online` | Usagi - Yasei no Toupai Online | typex | works (user: 100%), offline mode, no cards (Taito G-Net cards: TODO), touch = mouse or gun with a pointer dot | not in the test |  |
| `wontertainment-music-channel` | Won!Tertainment Music Channel | typex | works (user: 100%), touch = mouse or gun with a pointer dot, virtual cards on an overlay | not in the test |  |
| `tetris-the-grand-master-3` | Tetris The Grand Master 3 Terror-Instinct | typex | works (user: perfect) | not in the test |  |
| `street-fighter-iv` | Street Fighter IV | typex | works (user: perfect), intro video plays | not in the test |  |
| `super-street-fighter-iv-ae` | Super Street Fighter IV Arcade Edition (export, standalone) | typex | works (user: 100%), intro video plays | not in the test |  |
| `super-street-fighter-iv-ae-cabinet` | Super Street Fighter IV Arcade Edition | typex | works (user: 100%) | not in the test |  |
| `super-street-fighter-iv-ae-2012` | Super Street Fighter IV Arcade Edition Ver. 2012 | typex | works (user: 100%) | not in the test |  |
| `valve-limit-r` | Valve Limit R | typex | works (user), wheel, pedals, races start at once; TODO: option to hide the passenger girl's cut-ins | not in the test |  |
| `d1gp-arcade` | D1GP Arcade | typex | works (user: wheel, pedals, buttons, test menu); TODO: force feedback check on a wheel, magnetic card reader (saves) | not in the test |  |
| `wacky-races` | Wacky Races | typex | works (user: controls, start, view) | not in the test |  |
| `battle-gear-4` | Battle Gear 4 | typex | works (user: 100%) | not in the test |  |
| `battle-gear-4-tuned` | Battle Gear 4 Tuned | typex | works (user: wheel, pedals, buttons, intro, music, coins), test menu | not in the test |  |
| `battle-gear-4-tuned-pro` | Battle Gear 4 Tuned (professional) | typex | works (user: H shifter, clutch, coins in the I/O test) | not in the test |  |
| `harikiri-professional-baseball` | Harikiri Professional Baseball | typex | works (user: 100%), offline mode, touch = mouse or gun with a pointer dot | not in the test |  |
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
| `under-night-in-birth-exe-late-cl-r` | Under Night In-Birth Exe:Late[cl-r] | apm3 | works (user: game fine, 2026-11-05); Aime cards + saves headless (2026-11-05); 1 player per cabinet; no network versus | not in the test |  |
| `blade-strangers-apm3` | Blade Strangers (APM3) | apm3 | user 100% (2026-11-05), network banner removed, test menu, 1 player per cabinet, Aime cards + player data (ABAAS game server, user 100%) | not in the test |  |
| `senxin-aleste` | Senxin Aleste | apm3 | user 100% (2026-11-06); headless: attract, coins, Aime cards (records, settings, rankings saved and reloaded), story play, test mode | not in the test |  |
| `otoshu-dx` | Otoshu DX | apm3 | user 100% (2026-11-06), free play, touch dot; headless: touch screen (mouse / gun), Aime cards (name entry, login, data reloaded), songs, test mode | not in the test |  |
| `kasiori` | Kasiori | apm3 | user 100% (2026-11-06), link reported available; headless: Aime cards, story play, test mode | not in the test |  |
| `nosferatu-lilinor` | Nosferatu Lilinor | apm3 | user 100% (2026-11-06), no play sequence id; headless: Aime cards (name entry), play, test mode | not in the test |  |
| `pengo-online` | Pengo! Online | apm3 | user 100% (2026-11-06): touch menus (red dot), Aime cards, play, test mode, no link banner | not in the test |  |
| `puyo-puyo-e-sports` | Puyo Puyo e-Sports | apm3 | user 100% (2026-11-06), icon green; headless: Aime cards (terms, saved), play, test mode | not in the test |  |
| `milkchan` | Milkchan | apm3 | user 100% (2026-11-06); headless: Aime cards, story, test mode | not in the test |  |
| `koihime-enbu-ryorairai-v3` | Koihime Enbu RyoRaiRai Version 3 | apm3 | user 100% (2026-11-06); headless: Aime cards, modes, character select, test mode; no network versus | not in the test |  |
| `goonya-fighter` | Goonya Fighter | apm3 | user 100% (2026-11-06); headless: Aime cards, play, test mode | not in the test |  |
| `shanghai-iv-ryuumon-hiyaku` | Shanghai IV Ryuumon Hiyaku | apm3 | user 100% (2026-11-06); headless: Aime cards (player saved), play, test mode | not in the test |  |
| `rolling-gunner` | Rolling Gunner | apm3 | user 100% (2026-11-06), full screen (-f); headless: Aime cards, tutorial, test mode | not in the test |  |
| `cotton-rock-n-roll` | Cotton Rock 'n' Roll | apm3 | user 100% (2026-11-07); headless: Aime cards, story, play, game over saved (ranking), test mode | not in the test |  |
| `dead-or-alive-6` | Dead or Alive 6 | apm3 | user 100% (2026-11-09); headless: Aime cards (player saved at game over, reloaded), solo and in-store versus against the CPU, test mode and back, network icon green; no network versus | not in the test |  |
| `umihara-kawase-fresh` | Umihara Kawase Fresh! for AC | apm3 | user 100% (2026-11-08), jump on b1 (left Ctrl); headless: Aime cards (player loaded, rankings), Time Attack stage start, test mode | not in the test |  |
| `tapping-skill-test` | Tapping Skill Test | apm3 | user ok (2026-11-08): touch (mouse / gun), pinch = mouse wheel on the word (b2 / b3); headless: Aime cards (player loaded), minigames, test mode | not in the test |  |
| `virtua-fighter-3tb-online` | Virtua Fighter 3tb Online | apm3 | user 100% (2026-11-07), network icon green; headless: Aime cards (terms, user data, saved), team battle play, test mode and back | not in the test |  |
| `virtua-fighter-esports` | Virtua Fighter eSports | apm3 | headless (2026-11-07): Aime cards (terms, new player saved), guest and Aime arcade play, test mode (`vf5fs\vfes.exe`) and back, network icon green; no network versus | not in the test |  |
| `dengeki-bunko-fighting-climax-apm3` | Dengeki Bunko: Fighting Climax (APM3) | apm3 | user 100% (2026-11-06); headless: online, Aime cards, CPU fights | not in the test |  |
| `guilty-gear-xrd-rev2-apm3` | Guilty Gear Xrd REV 2 (APM3) | apm3 | user 100% (2026-11-06): Aime cards, the player saved and reloaded, test menu, full screen | not in the test |  |
| `guilty-gear-strive` | Guilty Gear -Strive- | apm3 | user 100% (2026-11-06): Aime cards, the player saved and reloaded, test menu, full screen; LOCAL plays alone against a still P2, no network versus | not in the test |  |
| `blazblue-cross-tag-battle` | BlazBlue: Cross Tag Battle (APM3) | apm3 | user 100% (2026-11-07): Aime cards, points, practice rooms against the CPU (Lv.1), player saved and reloaded, test menu; no network versus | not in the test |  |
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
| `sonic-dash-extreme` | Sonic Dash Extreme | seganu | headless tests (2026-11-08): attract, coins, start, character select, play, tickets dispensed during play and at the end, back to attract, test menu and back; not user-verified | not in the test |  |
| `luigis-mansion-arcade` | Luigi's Mansion Arcade | seganu | user (2026-11-08): "game is working perfect" (aim, start, play); headless tests: attract, coins, service, free play from a blank backup | not in the test |  |
| `project-diva-arcade-future-tone` | Hatsune Miku Project DIVA Arcade Future Tone | seganu | user (2026-11-16): "game working 100%" (picture, sound, play); headless tests: startup checks, attract (3D PVs), touch, new Aime card registration, a song played to its result (level up saved), next stage | not in the test |  |
| `mario-and-sonic-at-rio-2016-olympic-games` | Mario & Sonic at the Rio 2016 Olympic Games | alls | headless tests (2026-11-08): power-on test, attract, coins, mode / character / event select, 100m race to the result, test menu input test; user (2026-11-08): Aime name registration, two card sessions, the save kept | not in the test |  |
| `initial-d-arcade-stage-zero` | Initial D ARCADE STAGE Zero | alls | user 100% (2026-11-10), English option user OK; headless tests: attract, guest story race, new Aime player (registration, team, car, race, saved), returning player loaded, test menu network test | not in the test |  |
| `initial-d-arcade-stage-zero-ver-2` | Initial D ARCADE STAGE Zero Ver.2 | alls | user 100% (2026-11-10), English; headless tests: guest race, new Aime player saved, returning player loaded | not in the test |  |
| `chunithm-star` | CHUNITHM STAR | alls | user (2026-11-09): "working 100%" (Aime login, play, audio, touchpad slider); headless tests: power-on test, card registration, name entry, tutorial | not in the test |  |
| `chunithm-star-plus` | CHUNITHM STAR PLUS | alls | user (2026-11-09): "working 100%"; headless tests: card login, registration, name entry | not in the test |  |
| `chunithm-plus` | CHUNITHM PLUS | alls | headless tests (2026-11-09): attract, card login, tutorial with the slider, billing over TLS 1.0 | not in the test |  |
| `chunithm` | CHUNITHM | alls | user (2026-11-12): "game works fine" (Aime card created); headless tests: attract, free play, card login, registration, tutorial with the slider | not in the test |  |
| `chunithm-air` | CHUNITHM AIR | alls | headless tests (2026-11-12): attract, free play, card login, registration, tutorial with the slider | not in the test |  |
| `chunithm-air-plus` | CHUNITHM AIR PLUS | alls | headless tests (2026-11-12): attract, free play, card login, registration, trial play, name entry, tutorial | not in the test |  |
| `chunithm-amazon` | CHUNITHM AMAZON | alls | headless tests (2026-11-12): attract, free play, card login, registration, tutorial with the slider | not in the test |  |
| `chunithm-amazon-plus` | CHUNITHM AMAZON PLUS | alls | headless tests (2026-11-12): attract, free play, card login, registration, tutorial with the slider | not in the test |  |
| `chunithm-crystal` | CHUNITHM CRYSTAL | alls | headless tests (2026-11-12): attract, free play, card login, registration, tutorial with the slider | not in the test |  |
| `chunithm-crystal-plus` | CHUNITHM CRYSTAL PLUS | alls | headless tests (2026-11-12): attract, free play, card login, registration, trial play, name entry, tutorial; with SEGAguide's unpacked executables (the dump's are protected) | not in the test |  |
| `chunithm-paradise-lost` | CHUNITHM PARADISE LOST | alls | headless tests (2026-11-10): title server, card login, tutorial with the slider; with SEGAguide's unpacked executables (the dump's are protected) | not in the test |  |
| `crossbeats-rev-sunrise` | crossbeats REV. SUNRiSE | alls | user (2026-11-12): "the game seem to work perfectly" (mouse touch, red dot, Aime login with PIN, profile load); headless tests: boot, ALL.Net, game server tables, coins, touch, card sign-up | not in the test |  |
| `wonderland-wars` | Wonderland Wars | alls | user (2026-11-13, desktop): boots, touchpad pen OK, guest play; headless: power-on and network tests, shader cache | not in the test |  |
| `chunithm-superstar` | CHUNITHM SUPERSTAR | alls | not running (2026-11-09): protected executables, amdaemon dies under Wine (direct system calls) | not in the test |  |
| `chunithm-new` | CHUNITHM NEW!! | alls | user (2026-11-16, desktop): "game working 100%" (mouse with red dot, menus, Aime card); headless: power-on from a fresh backup, card login, new player, tutorial with the slider; with SEGAguide's unpacked 2.02 executables (the dump's are protected) | not in the test |  |
| `ongeki` | O.N.G.E.K.I. | alls | headless tests (2026-11-15): power-on test, attract, new Aime player (registration, name entry, news, story chapter 0) | not in the test |  |
| `ongeki-plus` | O.N.G.E.K.I. PLUS | alls | headless tests (2026-11-15): power-on, new Aime player, story, tutorial song (55-60 FPS) | not in the test |  |
| `ongeki-summer` | O.N.G.E.K.I. SUMMER | alls | headless tests (2026-11-15): power-on, new Aime player, story, tutorial song (57-60 FPS) | not in the test |  |
| `ongeki-bright-memory` | O.N.G.E.K.I. bright MEMORY | alls | headless tests (2026-11-16, dump 1.45): boot, title server (data 1.45.00, options A001-A028), new Aime player (name entry, login bonus, story, tutorial song) to the save | not in the test |  |
| `ongeki-re-fresh-152` | O.N.G.E.K.I. Re:Fresh 1.52 | alls | headless tests (2026-11-15): boot, title server (data 1.50.00, plain requests), new Aime player to the save | not in the test |  |
| `kemono-friends-3-planet-tours` | Kemono Friends 3 Planet Tours | alls | user (2026-11-16, desktop): "game 100% working"; headless: self test, attract, new Aime player (name, tutorial, card printed, saved), returning player, card scene, battle | not in the test |  |
| `teratoma` | Teratoma: The Last Rebellion | ringedge | works (user: 100%) | not in the test |  |
| `chaos-code-ringwide` | Chaos Code (RingWide) | ringedge | works (user: 100%) | not in the test |  |
| `ko-drive` | K.O. Drive | ringedge | works (user: 100%) | not in the test |  |
| `sega-racing-classic` | SEGA Racing Classic | ringedge | works (user: 100%) | not in the test |  |
| `sonic-sega-all-stars-racing` | Sonic & Sega All-Stars Racing | ringedge | works (user: 100%) | not in the test |  |
| `melty-blood-actress-again-current-code-ringwide` | Melty Blood Actress Again Current Code (RingWide) | ringedge | works (user: 100%), 4:3 | not in the test |  |
| `initial-d-arcade-stage-6` | Initial D Arcade Stage 6 AA | ringedge | works (user: full screen, inputs, cards, offline); force feedback not felt on a wheel | not in the test |  |
| `initial-d-arcade-stage-7` | Initial D Arcade Stage 7 AAX | ringedge | works (full screen, cards, offline) | not in the test |  |
| `initial-d-arcade-stage-8` | Initial D Arcade Stage 8 Infinity | ringedge | works (full screen, cards, offline) | not in the test |  |
| `pleasant-goat-and-big-big-wolf-happy-happy-playground` | Pleasant Goat and Big Big Wolf Happy Happy Playground | ringedge | title, attract and coins verified; gameplay, sound, payout not verified | not in the test |  |
| `puyo-puyo-quest-arcade` | Puyo Puyo!! Quest Arcade | ringedge | user 100% (2026-11-13, desktop); headless: coins, login, map | not in the test |  |
| `transformers-shadows-rising` | Transformers: Shadows Rising | segashell | works (user: 100%), test menu (user) | not in the test |  |
| `revolt` | Re-Volt (Tsunami cabinet) | tsunami | works (user: 100%): the cabinet's shell `Tsunami\TVTSHELL.EXE` (dump `games/arcadepc/revolt`), its console then `ReVolt.exe -coinop`; coin then the gas pedal starts a race; wheel, pedals and cabinet buttons through the TsuInput object (GetJoyInfo) | not in the test |  |
| `storm-racer-g` | Storm Racer G | wahlap | works (user: 100%) | not in the test |  |
| `storm-rider` | Storm Rider | wahlap | works (user: 100%) | not in the test |  |
| `ford-racing-full-blown` | Ford Racing Full Blown | europar | works (user: 100%) | not in the test |  |
| `sega-rally-3` | SEGA Rally 3 | europar | works (user: 100%) | not in the test |  |
| `showdown` | Showdown | segashell | works (user: 100%) | not in the test |  |
| `grid` | GRID | europar | works (user: 100%) | not in the test |  |
| `daytona-championship-usa` | Daytona Championship USA | europar | works (user: 100%); the manual gearbox not engaged (the race in automatic) | not in the test |  |
| `daytona-championship-usa-new-season-edition` | Daytona Championship USA New Season Edition | europar | works (user: 100%); the manual gearbox not engaged (the race in automatic) | not in the test |  |
| `men-in-black` | Men In Black | europar | works (headless test: attract, coins and free play, both players, guns aimed and shooting; user played on the desktop) | not in the test |  |
| `mission-impossible-arcade` | Mission: Impossible Arcade | europar | works (headless test: attract, coins and free play, both players, guns aimed, both triggers shooting in a mission, mission 3 and a later stage chosen) | not in the test |  |
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
| `castlevania-the-arcade` | Castlevania: The Arcade | konami | works (user: 100%), Little Witch unlocked; attract movie black | not in the test |  |
| `silent-hill-the-arcade` | Silent Hill: The Arcade | konami | works (user: 100%) | not in the test |  |
| `action-deka` | Action Deka | konami | works (user: 100%) | not in the test |  |
| `hyper-sports-2011-remake` | Hyper Sports 2011 Remake | konami | works (user: 100%) | not in the test |  |
| `thrill-drive-4` | Thrill Drive 4: Crazy Streets | konami | works (user: 100%) | not in the test |  |
| `chase-chase-jokers` | Chase Chase Jokers | konami | works (user: 100%) | not in the test |  |
| `galaxy-rangers` | Galaxy Rangers | arcadepc | works (headless tests: coins, start, level select, aim and shots, settings menu), 4 guns | not in the test |  |
| `quick-shot-arcade` | Quick Shot Arcade | arcadepc | user 100% (2026-10-26: display, crosshair, logo, bezel), 1 gun | not in the test |  |
| `top-gun-arcade` | Top Gun Arcade | arcadepc | user 100% (2026-10-26: display, crosshair, settings menu, bezel), 1 gun | not in the test |  |
| `happy-water-sprite-2` | Happy Water Sprite II | arcadepc | user 100% (2026-10-26), 2 water guns | not in the test |  |
| `mech-tornado` | Mech Tornado | arcadepc | user 100% (2026-10-26: laser mode, 4 players), 4 guns | not in the test |  |
| `fire-hero` | Fire Hero | arcadepc | user 100% (2026-10-26), 2 water cannons | not in the test |  |
| `zombie-crisis` | Zombie Crisis | arcadepc | user 100% (2026-10-26), 2 guns | not in the test |  |
| `bullseye-crack-shot` | Bullseye Crack Shot | arcadepc | user 100%, 1 gun | not in the test |  |
| `hot-shots-fire-rescue` | Hot Shots Fire Rescue | arcadepc | user 100% (2026-10-26), 4 water cannons | not in the test |  |
| `frenzy-express` | Frenzy Express | arcadepc | user 100% (2026-11-02) | not in the test |  |
| `hog-wild` | Hog Wild | arcadepc | user 100% (2026-11-05) | not in the test |  |
| `space-warp-66` | Space Warp 66 | arcadepc | user 100% (2026-11-06), 1 spinner, bezel | not in the test |  |
| `pigs-might-fly` | Pigs Might Fly | arcadepc | user 100% (2026-11-06) | not in the test |  |
| `wasteland-racers-2071` | Wasteland Racers 2071 | arcadepc | user 100% (2026-11-05) | not in the test |  |
| `polar-wars` | Polar Wars | arcadepc | works (user: 100%), 2 guns, last stage unlocked | not in the test |  |
| `skull-of-shadow` | Skull of Shadow | arcadepc | user 100% (2026-10-26), 4 guns | not in the test |  |
| `dino-invasion` | Dino Invasion | unis | user 100% (2026-10-26), 4 guns | not in the test |  |
| `after-dark` | After Dark | unis | user 100% (2026-11-03), 2 guns | not in the test |  |
| `after-dark-2-3` | After Dark (2.3) | unis | user ok (2026-11-03), 2 guns; 2026-11-07 user: operator menu, sound without saturation | not in the test |  |
| `night-hunter-after-dark-chapter-ii` | Night Hunter: After Dark Chapter II | unis | user 100% (2026-11-03), 2 guns | not in the test |  |
| `night-hunter-after-dark-chapter-ii-3-0-2` | Night Hunter: After Dark Chapter II (3.0.2) | unis | user 100% (2026-11-03), 2 guns, 4 missions selectable (Lost Trails) | not in the test |  |
| `raccoon-rampage` | Raccoon Rampage | unis | user 100% (2026-11-05), 4 guns | not in the test |  |
| `crazy-ride` | Crazy Ride | unis | user 100% (2026-11-05), wheel, force feedback | not in the test |  |
| `crazy-speed-arcade` | Crazy Speed Arcade | unis | user 100% (2026-11-05), wheel | not in the test |  |
| `elevator-action-invasion` | Elevator Action Invasion | unis | user 100% (2026-11-05), 2 guns, virtual elevator doors and bezel, every level unlocked | not in the test |  |
| `wicked-tuna` | Wicked Tuna | unis | user 100% (2026-11-05), 4 rods on light guns or pads | not in the test |  |
| `fantasy-soccer` | Fantasy Soccer | unis | user 100% (2026-11-05), 4 players | not in the test |  |
| `mighty-moto` | Mighty Moto | unis | user 100% (2026-11-05) | not in the test |  |
| `big-foot-crush` | Big Foot Crush | unis | user 100% (2026-11-06) | not in the test |  |
| `skylanders-cloud-patrol` | Skylanders Cloud Patrol | adrenaline | user 100% (2026-11-02), guns as touches | not in the test |  |
| `rabbids-hollywood` | Rabbids Hollywood | adrenaline | user 100% (2026-11-02), 4 guns (Sixense trackers emulated) | not in the test |  |
| `tomb-raider` | Tomb Raider | adrenaline | user 100% (2026-11-03), 4 guns (Sixense trackers emulated), operator settings saved | not in the test |  |
| `drakon-realm-keepers` | Drakon: Realm Keepers | adrenaline | user 100% (2026-11-03), 2 turrets | not in the test |  |
| `rampage` | Rampage | adrenaline | user 100% (2026-11-02), 3 players | not in the test |  |
| `hot-wheels-kings-of-the-road` | Hot Wheels: Kings of the Road | adrenaline | works headless (2026-11-02), 4 of 6 seats | not in the test |  |
| `need-for-speed-heat-takedown` | Need for Speed Heat Takedown | adrenaline | user 100% (2026-11-03), 1 wheel, force feedback, render scale | not in the test |  |
| `hot-wheels-victory-lap` | Hot Wheels: Victory Lap | adrenaline | user 100% (2026-11-04), 1 wheel, force feedback | not in the test |  |
| `infinity-blade-fx` | Infinity Blade FX | adrenaline | works headless (2026-11-03: coins, start, fight, gun swipes, shield, dodges), 1 player | not in the test |  |
| `fruit-ninja-fx` | Fruit Ninja FX | adrenaline | user 100% (2026-11-05), gun slices, decrypted executable | not in the test |  |
| `fruit-ninja-fx2` | Fruit Ninja FX 2 | adrenaline | user 100% (2026-11-04), gun slices | not in the test |  |
| `jetpack-joyride-arcade` | Jetpack Joyride Arcade | adrenaline | user 100% (2026-11-04), 1 player | not in the test |  |
| `spider-man-homecoming` | Spider-Man Homecoming | adrenaline | user 100% (2026-11-02), 2 buttons, portrait | not in the test |  |
| `candy-crush-arcade` | Candy Crush Arcade | adrenaline | works (user: 100%), portrait | not in the test |  |
| `crossy-road` | Crossy Road | adrenaline | user 100% (2026-11-03), 2 buttons, portrait (1080x1920 desktop fitted to the screen) | not in the test |  |
| `disney-crossy-road` | Disney Crossy Road | adrenaline | user 100% (2026-11-03), 2 buttons, portrait (64-bit, wal-loader64), stretched bezel | not in the test |  |
| `shooty-skies` | Shooty Skies | adrenaline | user 100% (2026-11-03), 2 joysticks (the game's keys) + buttons, portrait (64-bit), stretched bezel | not in the test |  |
| `flappy-tickets` | Flappy Tickets | adrenaline | user 100% (2026-11-03), 2 buttons, portrait, stretched bezel | not in the test |  |
| `mech-dinosaur` | Mech Dinosaur | arcadepc | user 100% (2026-10-26, all maps open), 4 guns | not in the test |  |
| `wisdom-zombies` | Wisdom Zombies | arcadepc | user 100% (2026-10-26, all maps open), 4 water guns | not in the test |  |
| `top-gun-2` | Top Gun 2 | arcadepc | works (headless tests 2026-11-04: coins, start, aim and shots scoring, operator menu; user: display, crosshairs; 2026-11-05 user: levels full screen in landscape), 2 camera guns, landscape option | not in the test |  |
| `galaxian-3` | Galaxian 3 | arcadepc | user 100% (2026-11-09), 4 guns (of the cabinet's 6) | not in the test |  |
| `burning-striker` | Burning Striker | arcadepc | user 100% (2026-11-09), 2 players (stick, 3 buttons) | not in the test |  |
| `come-on-baby` | Come on Baby! | arcadepc | user 100% (2026-11-14), 2 players (stick, 2 buttons, start) | not in the test |  |
| `come-on-baby-2` | Come On Baby! 2 Winter Championship | arcadepc | user 100% (2026-11-09), 2 players (stick, 2 buttons, start) | not in the test |  |
| `robin-hood` | Robin Hood | arcadepc | user 100% (2026-11-09), 1 gun | not in the test |  |
| `akuma-mortis-immortal` | Akuma Mortis Immortal | arcadepc | user 100% (2026-11-09), 2 guns | not in the test |  |
| `dcop` | DCOP | arcadepc | user 100% (2026-11-09), 1 gun | not in the test |  |
| `ballistics` | Ballistics | arcadepc | user 100% (2026-11-09), 1 player (steering, throttle, buttons) | not in the test |  |
| `bug-busters` | Bug Busters | arcadepc | user 100% (2026-11-09), 2 guns | not in the test |  |
| `arctic-thunder` | Arctic Thunder | arcadepc | user 100% (2026-11-09), 1 player (handlebar, throttle, attack) | not in the test |  |
| `ultimate-arctic-thunder` | Ultimate Arctic Thunder | arcadepc | user 100% (2026-11-10), 1 player (handlebar, throttle, attack) | not in the test |  |
| `hydro-thunder` | Hydro Thunder | arcadepc | user 100% with VCThunder 0.5 (2026-11-10), 1 player (wheel, throttle, boost, 3 views) | not in the test |  |
| `offroad-thunder` | Offroad Thunder | arcadepc | user 100% with VCThunder 0.5 (2026-11-10), 1 player (wheel, pedals, nitro, 3 cameras, shifter) | not in the test |  |
| `wild-west-shoot-out` | Wild West Shoot Out | arcadepc | user 100% (2026-11-10), 2 guns | not in the test |  |
| `mars-sortie` | Mars Sortie | arcadepc | user 100% (2026-11-10), 4 guns | not in the test |  |
| `family-guy-bowling-arcade` | Family Guy Bowling Arcade | arcadepc | user 100% (2026-11-10), 1 trackball | not in the test |  |
| `bounty-hunter` | Bounty Hunter | arcadepc | user 100% (2026-11-10), 1 player (8 pads) | not in the test |  |
| `friction` | Friction | arcadepc | user 100% (2026-11-10), 2 guns | not in the test |  |
| `house-of-the-dead-2-remake` | The House of the Dead 2: Remake | arcadepc | user 100% (2026-11-10), 2 guns | not in the test |  |
| `mad-bullets` | Mad Bullets | arcadepc | user 100% (2026-11-10), 1 gun | not in the test |  |
| `pull-the-trigger` | Pull The Trigger | arcadepc | user 100% (2026-11-10), 1 gun | not in the test |  |
| `the-house-of-the-dead-3` | The House of the Dead 3 | arcadepc | user 100% (2026-11-10), 2 guns | not in the test |  |
| `the-house-of-the-dead-remake` | The House of the Dead: Remake | arcadepc | user 100% (2026-11-10), 2 guns | not in the test |  |
| `plants-vs-zombies-the-last-stand` | Plants vs. Zombies: The Last Stand | arcadepc | user 100% (2026-11-10), 1 gun | not in the test |  |
| `savage-quest` | Savage Quest | arcadepc | user 100% (2026-11-10), 1 player | not in the test |  |
| `pop-moto` | Pop Moto | arcadepc | user 100% (2026-11-10), 1 player (handlebar, throttle, item) | not in the test |  |
| `senninha-gp` | Senninha GP | arcadepc | user 100% (2026-11-10), 1 player (wheel, pedals, boost, item, camera) | not in the test |  |
| `subway-surfers-arcade` | Subway Surfers Arcade | arcadepc | user 100% (2026-11-10), 1 player | not in the test |  |
| `extreme-off-road` | Extreme Off Road | arcadepc | user 100% (2026-11-12), 1 player | not in the test |  |
| `fix-it-felix-jr` | Fix-It Felix Jr. | arcadepc | user 100% (2026-11-12), 1 player (2 in turn) | not in the test |  |
| `in-the-groove-2` | In The Groove 2 | arcadepc | user 100% (2026-11-12), 2 players | not in the test |  |
| `monsterous` | Monsterous | arcadepc | user 100% (2026-11-12), 1 player | not in the test |  |
| `rerave` | ReRave | arcadepc | user 100% (2026-11-12), 1 touch (players 1 and 2 on it) | not in the test |  |
| `deathsmiles-ii` | Deathsmiles II | arcadepc | user 100% (2026-11-12), 2 players | not in the test |  |
| `omatsuri-yasan-kingyo-sukui` | Omatsuri Yasan Kingyo Sukui | arcadepc | user 100% (2026-11-15), 1 player | not in the test |  |
| `omatsuri-yasan-ganso-takoyaki` | Omatsuri Yasan Ganso Takoyaki | arcadepc | user 100% (2026-11-15), 1 player | not in the test |  |
| `quake-arcade-tournament-edition` | Quake: Arcade Tournament Edition | arcadepc | user 100% (2026-11-15), 1 player | not in the test |  |
| `dream-hunting` | Dream Hunting | arcadepc | user 100% (2026-11-12), 1 gun | not in the test |  |
| `photo-finish-racing` | Photo Finish Racing | arcadepc | user 100% (2026-11-12), 3 players | not in the test |  |
| `house-of-the-dead-overkill` | The House of the Dead: Overkill | arcadepc | user 100% (2026-11-10), 2 guns | not in the test |  |
| `left-4-dead-survivors` | Left 4 Dead: Survivors | nesica | user 100% (2026-11-16), NESiCA card (new player, CPU play), mouse and pad | not in the test |  |
| `groove-coaster-2` | Groove Coaster 2: Heavenly Festival | nesica | user 100% (2026-11-16), boosters, NESiCA card (registration, saves), free play | not in the test |  |
| `mario-and-sonic-at-tokyo-2020-olympic-games` | Mario & Sonic at the Tokyo 2020 Olympic Games | alls | user 100% (2026-11-08) | not in the test |  |
| `house-of-the-dead-scarlet-dawn` | The House of the Dead: Scarlet Dawn | alls | user 100% (2026-11-08), 2 guns | not in the test |  |
| `sega-world-drivers-championship` | Sega World Drivers Championship | alls | user-tested (2026-11-08): power-on test all GOOD, card registration with name entry, a race, the save, the returning player's licence | not in the test |  |
| `initial-d-the-arcade-season-3` | Initial D THE ARCADE Season 3 | alls | user 100% (2026-11-09), online: Aime cards, saves, story and time trial | not in the test |  |
| `initial-d-the-arcade-season-5` | Initial D THE ARCADE Season 5 | alls | headless tests (2026-11-11): attract, story and time trial races to the finish; user: races slow on an integrated GPU (the game runs one physics step per frame), box test pending | not in the test |  |
| `chrono-regalia` | Chrono Regalia | alls | user 100% (2026-11-10): guest and Aime card play | not in the test |  |
| `xtom-3d` | Xtom 3D | arcadepc | user 100% (2026-11-09) | not in the test |  |
| `sega-rally-2` | Sega Rally 2 | arcadepc | user 100% (2026-11-12), wheel | not in the test |  |
| `cut-the-rope` | Cut The Rope | arcadepc | user 100% (2026-11-13) | not in the test |  |
| `go-go-drummer-babe` | Go Go Drummer Babe | arcadepc | user 100% (2026-11-13) | not in the test |  |
| `radikal-bikers` | Radikal Bikers | arcadepc | user 100% (2026-11-14) | not in the test |  |
| `project-diva-arcade` | Hatsune Miku Project DIVA Arcade | ringedge | user 100% (2026-11-15, desktop) | not in the test |  |
| `border-break-scramble` | Border Break Scramble | ringedge2 | user (2026-11-15): "working 100% excluding online mode" | not in the test |  |
| `kancolle-arcade` | KanColle Arcade | seganu | user 100% (2026-11-15): guest and Aime card play | not in the test |  |
| `crossbeats-rev` | crossbeats REV. | alls | user 100% (2026-11-16, desktop): mouse with the red dot, touchpad multi-touch | not in the test |  |
<!-- GAMELIST END -->

## Thanks

Thanks to tovarichtch (https://github.com/tovarichtch/) for a prototype of light gun input over
TCP: it gave the idea of this loader's input design, read on the Linux side and forwarded over
TCP to the game in Wine.

Thanks to these projects, used as documentation and implementation references:

* Loaders and I/O
  * spice2x (Konami e-Amusement loader: ACIO devices, AVS, per-game hooks): https://github.com/spice2x/spice2x.github.io
  * bananatools (Bandai Namco ES3: AMCUS, Banapass reader, NA-JV): https://gitea.tendokyu.moe/TeamTofuShop/bananatools
  * segatools (Sega ALLS / Nu / RingEdge: keychip, Aime reader, JVS): https://gitea.tendokyu.moe/TeamTofuShop/segatools
  * taitools (Taito Type X3 / NESiCAxLive): https://gitea.tendokyu.moe/TeamTofuShop/taitools
  * DemulShooter (light gun games): https://github.com/argonlefou/DemulShooter
  * FFBArcadePlugin (force feedback): https://github.com/Boomslangnz/FFBArcadePlugin
  * OpenBanapassW5P (Banapass for Wangan Midnight Maximum Tune 5): https://github.com/kritbualad/OpenBanapassW5P
* Game servers
  * ARTEMiS (ALL.Net server and per-title game servers): https://gitea.tendokyu.moe/Hay1tsme/artemis
  * AquaDX: https://github.com/MewoLab/AquaDX
  * OIAS: https://github.com/OtogameLab/OIAS
  * Bayshore (Wangan Midnight Maximum Tune servers): https://github.com/ProjectAsakura/Bayshore
    and its forks https://github.com/shiroikitsu8/Bayshore_legacy,
    https://github.com/kritbualad/Bayshore_WM5DX-PLUS, https://github.com/shadany7824/Bayshore_5,
    https://github.com/RumiHasegawa/Bayshore_WM5, https://github.com/anthonyng43/Bayshore_6r,
    https://github.com/JoeKoh27/Bayshore_6RR
  * FakeNesicaService (NESiCAxLive services): https://github.com/ArcadeMachinist/FakeNesicaService
  * GC-local-server-rewrite (NESiCA card server): https://github.com/asesidaa/GC-local-server-rewrite
  * Asphyxia CORE and its plugins (e-Amusement): https://github.com/asphyxia-core/core,
    https://github.com/asphyxia-core/plugins
  * bemaniutils (e-Amusement): https://github.com/DragonMinded/bemaniutils
  * hiiragi (e-Amusement): https://github.com/nennneko5787/hiiragi
  * IaSoC e-Amusement server: https://github.com/IaSoC/iasoc-eamusement-server
* Game data
  * MKDX-Unlocked-Profile (Mario Kart Arcade GP DX): https://github.com/Harmmmm/MKDX-Unlocked-Profile
  * injustice-arcade-cards (Injustice arcade): https://github.com/CB9001/injustice-arcade-cards
* Runtime
  * Wine: https://www.winehq.org
  * GE-Proton: https://github.com/GloriousEggroll/proton-ge-custom
  * DXVK: https://github.com/doitsujin/dxvk
  * gamescope: https://github.com/ValveSoftware/gamescope
  * dgVoodoo2: https://dege.freeweb.hu/dgVoodoo2/
  * ReShade: https://reshade.me


### Sinden lightgun frame

Lightgun games automatically enable a native Sinden frame when a Sinden is connected at
launch. The gun driver must already be running. The game's **Common** options include
**Sinden border** (`auto`, `1` always on, `0` off), **Border size**, and **Border color**.
These can also be set globally in `launcher.yaml` or per game in `userprofiles/`:

```yaml
env:
  WAL_SINDEN_BORDER: "auto"
  WAL_SINDEN_BORDER_SIZE: "medium"
  WAL_SINDEN_BORDER_COLOR: "white"
```

Size accepts Batocera's `thin` (1%), `medium` (2%), `big` (2% color plus 1% outer black),
or a decimal percentage of the screen width, from `0.1` to `10`. Color accepts `white`,
`red`, `green`, `blue`, or a quoted `#RRGGBB` value. The frame covers the monitor; the
complete game picture is scaled into its interior with its aspect preserved. Physical
gun and mouse aim is transformed to match. Existing aspect/stretch settings still apply
to the game picture inside that frame.

Bezel artwork and the game are composed together inside the frame, including Top Gun 2's
dynamic landscape bezel and separate portrait bezels.

Presentation uses the payload's native Direct3D 8/9/11 or OpenGL hooks, following
linuxloader's clean-frame approach. It adds GPU copies and a scaled draw while enabled;
there is no CPU pixel readback and no gamescope requirement. A performance impact has
not yet been measured on real games. A profile that already explicitly enables gamescope
continues to use its existing setting; the border itself never enables it.

Batocera's generator passes `controllers.guns.borderssize`,
`controllers.guns.borderscolor`, and `controllers.guns.bordersmode` (plus the game-specific
`bordersmode`) from system config. `hidden` disables the frame and `force` enables it;
auto activates only for a gun game with a detected Sinden. Optional overrides are
`wal_sinden_border`, `wal_sinden_border_size`, and `wal_sinden_border_color`.
`tools/batocera/install.sh` also updates the generator when installing into an RGS tree.
