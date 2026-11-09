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

## Games (305)

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
| `farcry-paradise-lost` | Far Cry Paradise Lost | globalvr | works (user: 100%) | pass | 2026-11-01 |
| `aliens-extermination` | Aliens Extermination | globalvr | works (user: 100%), 2 guns (trigger also starts), movies | pass | 2026-11-01 |
| `the-swarm` | The Swarm | globalvr | works (user: 100%), 2 guns (fire, grenade, shield); stage changes: the game started again with the next map (Relaunch message), works (user: 100%); new high scores not saved | pass | 2026-11-01 |
| `need-for-speed` | Need for Speed | globalvr | works (user: 100%) | pass | 2026-11-01 |
| `need-for-speed-gt` | Need for Speed GT | globalvr | works (user: 100%) | pass | 2026-11-01 |
| `need-for-speed-underground` | Need for Speed Underground | globalvr | works (user: 100%) | pass | 2026-11-01 |
| `need-for-speed-carbon` | Need for Speed Carbon | globalvr | works (user: 100%); force feedback not tested on a wheel | pass | 2026-11-01 |
| `nascar-racing` | NASCAR Racing | globalvr | works (user: 100%) | pass | 2026-11-01 |
| `nascar-team-racing` | NASCAR Team Racing | globalvr | works (user: 100%) | pass | 2026-11-01 |
| `blazing-angels` | Blazing Angels: Squadrons of WWII | globalvr | works (user: 100%) | pass | 2026-11-01 |
| `beach-head-2000` | Beach Head 2000 | globalvr | works (user: 100%) | pass | 2026-11-01 |
| `beach-head-2002` | Beach Head 2002 | globalvr | works (user: 100%) | pass | 2026-11-01 |
| `beach-head-2003-desert-war` | Beach Head 2003 Desert War | globalvr | works (user: 100%), machine gun, cannon, missiles | pass | 2026-11-01 |
| `twisted-nitro-stunt-racing` | Twisted: Nitro Stunt Racing | globalvr | works (user: 100%) | not in the test |  |
| `redline-rampage` | Redline Rampage | globalvr | works (user: 100%) | pass | 2026-11-01 |
| `justice-league-heroes-united` | Justice League: Heroes United | globalvr | works (user: 100%), 2 players | not in the test |  |
| `madden-nfl-season-2` | EA SPORTS Madden NFL Football Season 2 | globalvr | works (user: 100%) | pass | 2026-11-02 |
| `sin` | SiN | globalvr | works (user: 100%) | pass | 2026-11-01 |
| `heavy-gear-ii` | Heavy Gear II | globalvr | works (user: 100%) | pass | 2026-11-01 |
| `pga-tour-golf-championship-edition-iii` | PGA TOUR Golf Championship Edition III | globalvr | works (user: 100%) | pass | 2026-11-02 |
| `pga-tour-golf-championship-edition-ii` | PGA TOUR Golf Championship Edition II | globalvr | works (user: 100%) | not in the test |  |
| `pga-tour-golf` | PGA TOUR Golf | globalvr | works (user: 100%), trackball: the mouse or touchpad (or stick) | not in the test |  |
| `pga-tour-golf-team-challenge` | PGA TOUR Golf Team Challenge | globalvr | works (user: 100%), trackball: the mouse or touchpad (or stick) | not in the test |  |
| `ultrapin` | UltraPin | globalvr | works (user: 100%) | not in the test |  |
| `akai-katana-shin` | Akai Katana Shin | nesica | in game (GAME_START) | pass | 2026-11-01 |
| `aquapazza` | Aquapazza: Aquaplus Dream Match | nesica | works (user: 100%) | pass | 2026-11-01 |
| `arcana-heart-2` | Arcana Heart 2 | nesica | in game (user) | pass | 2026-11-01 |
| `arcana-heart-3-lmss` | Arcana Heart 3 Love Max Six Stars!!!!!! | nesica | in game (user, GAME_START) | pass | 2026-11-01 |
| `suggoi-arcana-heart-2` | Suggoi! Arcana Heart 2 | nesica | works (user: 100%) | pass | 2026-11-01 |
| `battle-fantasia-nesica` | Battle Fantasia (NESiCAxLive) | nesica | works (user: 100%) | pass | 2026-11-01 |
| `spica-adventure-nesica` | Spica Adventure (NESiCAxLive) | nesica | works (user: 100%) | pass | 2026-11-01 |
| `blazblue-central-fiction` | BlazBlue Central Fiction 2.01 | nesica | in game, NESiCA online | pass | 2026-11-01 |
| `blazblue-chronophantasma` | BlazBlue Chronophantasma 2.03 | nesica | works (user: 100%) | pass | 2026-11-01 |
| `blazblue-continuum-shift-2` | BlazBlue Continuum Shift II | nesica | works (user: 100%) | pass | 2026-11-01 |
| `chaos-breaker` | Chaos Breaker | nesica | in fight, music | pass | 2026-11-01 |
| `chaos-code-103` | Chaos Code: New Sign of Catastrophe 1.03 | nesica | in fight (user) | pass | 2026-11-01 |
| `chaos-code-211` | Chaos Code: New Sign of Catastrophe 2.11 | nesica | in fight (user) | pass | 2026-11-01 |
| `crimzon-clover` | Crimzon Clover | nesica | works fullscreen (user: 100%; 1280x800: pillarboxed on a 16:9 screen) | fail: picture | 2026-11-02 |
| `daemon-bride` | Daemon Bride: Additional Gain | nesica | in fight | pass | 2026-11-01 |
| `dariusburst-another-chronicle` | Dariusburst Another Chronicle (1.05) | nesica | works (user: 100%) | pass | 2026-11-01 |
| `dariusburst-another-chronicle-ex` | Dariusburst Another Chronicle EX | nesica | works (user: 100%), 4 players, sound, test menu in Japanese | pass | 2026-11-01 |
| `dark-awake` | Dark Awake: The King Has No Name | nesica | in fight | pass | 2026-11-01 |
| `do-not-fall` | Do Not Fall: Run for Your Drink | nesica | works (user) | pass | 2026-11-01 |
| `dragon-dance` | Dragon Dance | nesica | works (user), smoke and sparkle effects correct with d7vk; crash on applying saved display settings fixed by a code patch | pass | 2026-11-01 |
| `elevator-action` | Elevator Action Death Parade | nesica | works (user: 100%), 4:3 | pass | 2026-11-01 |
| `en-eins-perfektewelt` | EN-Eins Perfektewelt | nesica | works (user) | pass | 2026-11-01 |
| `exception` | Exception | nesica | works fullscreen (user) | pass | 2026-11-01 |
| `gouketsuji-ichizoku` | Gouketsuji Ichizoku: Matsuri Senzo Kuyou | nesica | works (user) | pass | 2026-11-01 |
| `homura` | Homura | nesica | works (user), sound effects and music in game; attract demo silent, as under Windows (original or dump, not the loader) | pass | 2026-11-01 |
| `hyper-street-fighter-2` | Hyper Street Fighter II: The Anniversary Edition | nesica | works (user) | pass | 2026-11-01 |
| `ikaruga` | Ikaruga | nesica | in game (user) | pass | 2026-11-01 |
| `kof-2002-um` | The King of Fighters 2002 Unlimited Match | nesica | works (user) | pass | 2026-11-01 |
| `kof-98-umfe` | The King of Fighters '98 Ultimate Match Final Edition | nesica | works (user) | pass | 2026-11-01 |
| `kof-xiii-climax` | The King of Fighters XIII Climax | nesica | in game, movies | pass | 2026-11-01 |
| `magical-beat` | Magical Beat | nesica | works (user) | pass | 2026-11-01 |
| `nitroplus-blasterz` | Nitroplus Blasterz: Heroines Infinite Duel | nesica | works (user) | pass | 2026-11-01 |
| `persona-4-arena` | Persona 4 The Ultimate in Mayonaka Arena | nesica | works (user: perfect) | pass | 2026-11-01 |
| `persona-4-ultimax` | Persona 4 The Ultimax Ultra Suplex Hold | nesica | works (user) | pass | 2026-11-01 |
| `psychic-force-2012` | Psychic Force 2012 | nesica | works (user) | pass | 2026-11-01 |
| `puzzle-bobble` | Puzzle Bobble | nesica | works (user) | pass | 2026-11-02 |
| `raiden-3` | Raiden III | nesica | in game (user); intro movie black | pass | 2026-11-01 |
| `raiden-4` | Raiden IV | nesica | in game (user), intro movie (with GE-Proton's winedmo MPEG sequence header fix, after 11-7) | pass | 2026-11-01 |
| `rastan-saga` | Rastan Saga | nesica | works (user) | pass | 2026-11-01 |
| `senko-no-ronde-duo` | Senko no Ronde DUO: Dis-United Order | nesica | works, sound effects (user) | pass | 2026-11-01 |
| `skullgirls-2nd-encore` | Skullgirls 2nd Encore | nesica | works (user) | pass | 2026-11-01 |
| `space-invaders` | Space Invaders | nesica | works (user: 100%) | fail: picture | 2026-11-02 |
| `strania` | Strania: The Stella Machina | nesica | works (user), music and sound effects | pass | 2026-11-01 |
| `street-fighter-3-3rd-strike` | Street Fighter III 3rd Strike: Fight for the Future | nesica | works (user) | pass | 2026-11-01 |
| `street-fighter-zero-3` | Street Fighter Zero 3 | nesica | works fullscreen (user) | pass | 2026-11-01 |
| `the-rumble-fish-2` | The Rumble Fish 2 | nesica | works (user) | pass | 2026-11-01 |
| `tottemo-e-mahjong` | Tottemo E Mahjong | nesica | works (user); test menu (TestMode.exe) crashes, TODO | pass | 2026-11-01 |
| `trouble-witches-ac` | Trouble Witches AC: Amalgam no Joutachi | nesica | works (user) | pass | 2026-11-01 |
| `ultra-street-fighter-4` | Ultra Street Fighter 4 | nesica | works (user: 100%), opening video | pass | 2026-11-01 |
| `vampire-savior` | Vampire Savior: The Lord of Vampire | nesica | works (user) | pass | 2026-11-01 |
| `yatagarasu` | Yatagarasu: Attack on Cataclysm | nesica | works (user: 100%), fullscreen with its side portraits, Japanese text | pass | 2026-11-01 |
| `million-arthur-arcana-blood` | Million Arthur Arcana Blood | nesica2 | works (user: 100%: training and story modes), NESiCA network connected | pass | 2026-11-01 |
| `kof-xiv` | The King of Fighters XIV | nesica2 | works (user: 100%), NESiCA network connected | pass | 2026-11-01 |
| `snk-heroines` | SNK Heroines Tag Team Frenzy | nesica2 | works (user: 100%), NESiCA network connected | pass | 2026-11-01 |
| `samurai-shodown-130` | Samurai Shodown (1.30.00) | nesica2 | works (user: 100%), NESYS offline; a red banner ただいまネットワークを使ったプレイができません。 (network play unavailable) stays on screen: the game's own offline notice | not in the test |  |
| `samurai-shodown-180` | Samurai Shodown (1.80.00) | nesica2 | works (user: 100%), NESYS offline; a red banner ただいまネットワークを使ったプレイができません。 (network play unavailable) stays on screen: the game's own offline notice | pass | 2026-11-01 |
| `fighting-ex-layer` | Fighting EX Layer | nesica2 | in game (user: card accepted, game starts), NESiCA network connected; card saved (user: name shown, no freeze after a match) | pass | 2026-11-01 |
| `theatrhythm-final-fantasy-all-star-carnival` | Theatrhythm Final Fantasy All-Star Carnival | nesica2 | works (user: 100%), NESiCA cards (loader's card server, progress saved), touch screen (mouse, red pointer); right lever on I/J/K/L; no matching with other cabinets | pass | 2026-11-01 |
| `3d-cosplay-mahjong` | 3D Cosplay Mahjong | typex | in game (mahjong hand) | pass | 2026-11-01 |
| `battle-fantasia` | Battle Fantasia | typex | works (user: 100%) | pass | 2026-11-01 |
| `cyber-diver` | Cyber Diver | typex | works (user: 100%) | pass | 2026-11-01 |
| `sonic-blast-heroes` | Sonic Blast Heroes | typex | works (user: 100%) | pass | 2026-11-01 |
| `block-king-ball-shooter` | Block King Ball Shooter | typex | works (user: touch, coins, start, test menu; 4-player co-op as DemulShooter) | pass | 2026-11-01 |
| `blazblue-calamity-trigger` | BlazBlue Calamity Trigger | typex | in fight (user) | pass | 2026-11-01 |
| `blazblue-continuum-shift` | BlazBlue Continuum Shift | typex | works (user: 100%) | pass | 2026-11-01 |
| `chase-hq-2` | Chase H.Q. 2 | typex | works (user: 100%, wheel/pedals, switches, Nancy videos) | pass | 2026-11-01 |
| `gigawing-generations` | GigaWing Generations | typex | works (user), Landscape/Bezel dump rotated by its ReShade | pass | 2026-11-01 |
| `chaos-breaker-typex` | Chaos Breaker | typex | works (user: perfect) | pass | 2026-11-01 |
| `gaia-attack-4` | Gaia Attack 4 | typex | works (user: 100%, 4 guns, coins, sound, videos) | pass | 2026-11-01 |
| `gouketsuji-ichizoku-typex2` | Gouketsuji Ichizoku - Matsuri Senzo Kuyou | typex | works (user: 100%) | pass | 2026-11-01 |
| `kof-98-um-typex` | The King of Fighters '98 Ultimate Match | typex | works (user: perfect) | pass | 2026-11-01 |
| `kof-sky-stage` | The King of Fighters Sky Stage | typex | works (user), rotated by the dump's ReShade | pass | 2026-11-01 |
| `haunted-museum` | Haunted Museum | typex | works (user: 100%, guns, service/test, sound) | pass | 2026-11-01 |
| `gundam-spirits-of-zeon` | Mobile Suit Gundam: Spirits of Zeon | typex | works (user: guns, inputs, coins; intro video fixed with `WAL_D3D9_POW2`) | pass | 2026-11-01 |
| `gundam-spirits-of-zeon-2p` | Mobile Suit Gundam: Spirits of Zeon (2 players) | typex | works (user: perfect, in gamescope, guns) | pass | 2026-11-01 |
| `haunted-museum-2` | Haunted Museum II | typex | works (user: guns, inputs, sound, video) | pass | 2026-11-01 |
| `k-on-after-school-rhythm-selection` | K-On! After School Rhythm Selection | typex | works (headless tests: boot, card purchase, 3 virtual cards scanned, song select, rhythm game) | not in the test |  |
| `king-of-fighters-maximum-impact-regulation-a` | King of Fighters Maximum Impact Regulation A | typex | works (user: 100%), intro movie | pass | 2026-11-01 |
| `king-of-fighters-xii` | The King of Fighters XII | typex | in game, intro video | pass | 2026-11-01 |
| `elevator-action-death-parade` | Elevator Action Death Parade | typex | works (user: 100%, perfect), 2 guns, attraction doors and bezel, crosshairs option | pass | 2026-11-01 |
| `king-of-fighters-xiii` | The King of Fighters XIII | typex | works (user: 100%) | pass | 2026-11-01 |
| `kof-xiii-climax-typex2` | The King of Fighters XIII Climax (Type X2) | typex | works (user: 100%) | pass | 2026-11-01 |
| `music-gungun-2` | Music GunGun! 2 | typex | works (user: perfect), attract movie, sound, 2 guns | pass | 2026-11-01 |
| `raiden-3-typex` | Raiden III | typex | works (user); intro movie black (as NESiCA) | pass | 2026-11-01 |
| `raiden-4-typex` | Raiden IV | typex | works (user) | pass | 2026-11-01 |
| `shikigami-no-shiro-3` | Shikigami no Shiro III | typex | works (user), Landscape/Bezel dump rotated by its ReShade | pass | 2026-11-01 |
| `samurai-spirits-sen` | Samurai Spirits Sen | typex | works (user: 100%) | pass | 2026-11-01 |
| `senko-no-ronde-duo-typex2` | Senko no Ronde DUO: Dis-United Order | typex | works (user: perfect) | pass | 2026-11-01 |
| `spica-adventure` | Spica Adventure | typex | works (user: 100%) | pass | 2026-11-01 |
| `new-super-mario-bros-wii-coin-world` | New Super Mario Bros. Wii Coin World | typex | works (user: 100%), 4 satellites, medals, hoppers, satellite test menu | pass | 2026-11-01 |
| `taisen-hot-gimmick-5` | Taisen Hot Gimmick 5: Mirai Eigou | typex | works (user: 100%) | pass | 2026-11-01 |
| `usagi-online` | Usagi - Yasei no Toupai Online | typex | works (user: 100%), offline mode, no cards (Taito G-Net cards: TODO), touch = mouse or gun with a pointer dot | pass | 2026-11-02 |
| `wontertainment-music-channel` | Won!Tertainment Music Channel | typex | works (user: 100%), touch = mouse or gun with a pointer dot, virtual cards on an overlay | not in the test |  |
| `tetris-the-grand-master-3` | Tetris The Grand Master 3 Terror-Instinct | typex | works (user: perfect) | pass | 2026-11-01 |
| `street-fighter-iv` | Street Fighter IV | typex | works (user: perfect), intro video plays | pass | 2026-11-01 |
| `super-street-fighter-iv-ae` | Super Street Fighter IV Arcade Edition (export, standalone) | typex | works (user: 100%), intro video plays | pass | 2026-11-01 |
| `super-street-fighter-iv-ae-cabinet` | Super Street Fighter IV Arcade Edition | typex | works (user: 100%) | pass | 2026-11-01 |
| `super-street-fighter-iv-ae-2012` | Super Street Fighter IV Arcade Edition Ver. 2012 | typex | works (user: 100%) | pass | 2026-11-01 |
| `valve-limit-r` | Valve Limit R | typex | works (user), wheel, pedals, races start at once; TODO: option to hide the passenger girl's cut-ins | pass | 2026-11-01 |
| `d1gp-arcade` | D1GP Arcade | typex | works (user: wheel, pedals, buttons, test menu); TODO: force feedback check on a wheel, magnetic card reader (saves) | pass | 2026-11-01 |
| `wacky-races` | Wacky Races | typex | works (user: controls, start, view) | pass | 2026-11-01 |
| `battle-gear-4` | Battle Gear 4 | typex | works (user: 100%) | pass | 2026-11-01 |
| `battle-gear-4-tuned` | Battle Gear 4 Tuned | typex | works (user: wheel, pedals, buttons, intro, music, coins), test menu | pass | 2026-11-01 |
| `battle-gear-4-tuned-pro` | Battle Gear 4 Tuned (professional) | typex | works (user: H shifter, clutch, coins in the I/O test) | pass | 2026-11-01 |
| `harikiri-professional-baseball` | Harikiri Professional Baseball | typex | works (user: 100%), offline mode, touch = mouse or gun with a pointer dot | not in the test |  |
| `fast-and-furious` | The Fast and the Furious | rawthrills | works (user: 100%) | pass | 2026-11-01 |
| `fast-and-furious-drift` | The Fast and the Furious Drift | rawthrills | works (user: 100%) | pass | 2026-11-02 |
| `fast-and-furious-supercars` | The Fast and the Furious SuperCars | rawthrills | works (user: 100%) | pass | 2026-11-01 |
| `fast-and-furious-super-bikes` | The Fast and the Furious Super Bikes | rawthrills | works (user: 100%) | pass | 2026-11-01 |
| `fast-and-furious-super-bikes-2` | The Fast and the Furious Super Bikes 2 | rawthrills | works (user: 100%) | pass | 2026-11-01 |
| `dirty-drivin` | Dirty Drivin' | rawthrills | works (user: 100%) | pass | 2026-11-01 |
| `frogger` | Frogger (2011 redemption) | rawthrills | works (user: 100%), two-player controls, service menu, volume | pass | 2026-11-01 |
| `cars` | Cars | rawthrills | in game (user: accel, TEST, calibration) | pass | 2026-11-01 |
| `nicktoons-nitro-racing` | Nicktoons Nitro Racing | rawthrills | works (user: 100%) | pass | 2026-11-01 |
| `batman` | Batman | rawthrills | works (user-confirmed gameplay and controls; FFB wheel test pending) | pass | 2026-11-01 |
| `injustice-arcade` | Injustice Arcade | rawthrills | works (user: 100%, virtual card readers) | pass | 2026-11-01 |
| `teenage-mutant-ninja-turtles` | Teenage Mutant Ninja Turtles | rawthrills | works (user: 100%), four-player controls, test menu, variable-height jump, videos | fail: started | 2026-11-02 |
| `super-alpine-racer` | Super Alpine Racer | rawthrills | in progress: boots, a race played from scripted inputs (swing, start) | pass | 2026-11-01 |
| `x-games-snowboarder` | X-Games Snowboarder | rawthrills | in progress: boots, a race played from scripted inputs | pass | 2026-11-01 |
| `winter-x-games-snocross` | Winter X Games SnoCross | rawthrills | works (user: 100%) | pass | 2026-11-01 |
| `h2o-overdrive` | H2O Overdrive | rawthrills | works (user: 100%) | pass | 2026-11-01 |
| `guitar-hero-arcade` | Guitar Hero Arcade EDITION | rawthrills | works (user: 100%) | pass | 2026-11-01 |
| `virtua-tennis-4` | Virtua Tennis 4 | ringedge | works (user: 100%), test menu | pass | 2026-11-01 |
| `guilty-gear-xx-accent-core-plus-r` | Guilty Gear XX Accent Core Plus R | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `under-night-in-birth-exe-late` | Under Night In-Birth Exe:Late | ringedge | works (user: 100%), Aime offline banner removed; no Aime card play | pass | 2026-11-01 |
| `under-night-in-birth-exe-late-st` | Under Night In-Birth Exe:Late[st] | ringedge | works (user: 100%), Aime offline banner removed; no Aime card play | pass | 2026-11-01 |
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
| `dead-or-alive-6` | Dead or Alive 6 | apm3 | user 100% (2026-11-09); headless: Aime cards (player saved, reloaded), solo and in-store versus against the CPU, test mode | not in the test |  |
| `umihara-kawase-fresh` | Umihara Kawase Fresh! for AC | apm3 | user 100% (2026-11-08), jump on b1 (left Ctrl); headless: Aime cards (player loaded, rankings), Time Attack stage start, test mode | not in the test |  |
| `tapping-skill-test` | Tapping Skill Test | apm3 | user ok (2026-11-08): touch (mouse / gun), pinch = mouse wheel on the word (b2 / b3); headless: Aime cards (player loaded), minigames, test mode | not in the test |  |
| `dengeki-bunko-fighting-climax-apm3` | Dengeki Bunko: Fighting Climax (APM3) | apm3 | user 100% (2026-11-06); headless: online, Aime cards, CPU fights | not in the test |  |
| `guilty-gear-xrd-rev2-apm3` | Guilty Gear Xrd REV 2 (APM3) | apm3 | user 100% (2026-11-06): Aime cards, the player saved and reloaded, test menu, full screen | not in the test |  |
| `guilty-gear-strive` | Guilty Gear -Strive- | apm3 | user 100% (2026-11-06): Aime cards, the player saved and reloaded, test menu, full screen; LOCAL plays alone against a still P2, no network versus | not in the test |  |
| `blazblue-cross-tag-battle` | BlazBlue: Cross Tag Battle (APM3) | apm3 | user 100% (2026-11-07): Aime cards, points, practice rooms against the CPU (Lv.1), player saved and reloaded, test menu; no network versus | not in the test |  |
| `koihime-enbu` | Koihime Enbu | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `blade-arcus-from-shining` | Blade Arcus from Shining (2.10.00) | ringedge | works (user: 100%), Aime offline banner removed; no Aime card play | pass | 2026-11-01 |
| `blade-strangers` | Blade Strangers (1.02.00, 1.3.2) | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `caladrius-ac` | Caladrius AC | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `melty-blood-actress-again-current-code-re2` | Melty Blood Actress Again Current Code (RE2) | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `phantom-breaker-another-code` | Phantom Breaker: Another Code | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `ge-sen-love-plus-pengo` | Ge-Sen Love Plus Pengo! | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `transformers-human-alliance` | Transformers: Human Alliance | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `guilty-gear-xrd-rev2` | Guilty Gear Xrd Rev 2 | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `guilty-gear-xrd-sign` | Guilty Gear Xrd -SIGN- | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `guilty-gear-xrd-revelator` | Guilty Gear Xrd -REVELATOR- | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `dengeki-bunko-fighting-climax` | Dengeki Bunko: Fighting Climax | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `dengeki-bunko-fighting-climax-ignition` | Dengeki Bunko: Fighting Climax Ignition | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `dead-or-alive-5-ultimate-arcade` | Dead or Alive 5 Ultimate Arcade | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `under-defeat-hd-plus` | Under Defeat HD+ | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `lets-go-island-3d` | Let's Go Island 3D: Lost on the Island of Tropics | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `lets-go-island-ringwide` | Let's Go Island: Lost on the Island of Tropics (RingWide) | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `sega-golden-gun` | SEGA Golden Gun | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `dream-raiders` | Dream Raiders | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `operation-ghost` | Operation G.H.O.S.T. | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `target-bravo-operation-ghost` | Target Bravo: Operation G.H.O.S.T. | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `medal-no-gunman` | Medal no Gunman | ringedge | works (user: mouse) | pass | 2026-11-01 |
| `super-monkey-ball-ticket-blitz` | Super Monkey Ball Ticket Blitz | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `lets-go-safari` | Let's Go Safari | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `lady-luck` | Lady Luck | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `teratoma` | Teratoma: The Last Rebellion | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `chaos-code-ringwide` | Chaos Code (RingWide) | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `ko-drive` | K.O. Drive | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `sega-racing-classic` | SEGA Racing Classic | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `sonic-sega-all-stars-racing` | Sonic & Sega All-Stars Racing | ringedge | works (user: 100%) | pass | 2026-11-01 |
| `melty-blood-actress-again-current-code-ringwide` | Melty Blood Actress Again Current Code (RingWide) | ringedge | works (user: 100%), 4:3 | pass | 2026-11-01 |
| `initial-d-arcade-stage-6` | Initial D Arcade Stage 6 AA | ringedge | works (user: full screen, inputs, cards, offline); force feedback not felt on a wheel | pass | 2026-11-01 |
| `initial-d-arcade-stage-7` | Initial D Arcade Stage 7 AAX | ringedge | works (full screen, cards, offline) | pass | 2026-11-01 |
| `initial-d-arcade-stage-8` | Initial D Arcade Stage 8 Infinity | ringedge | works (full screen, cards, offline) | pass | 2026-11-01 |
| `pleasant-goat-and-big-big-wolf-happy-happy-playground` | Pleasant Goat and Big Big Wolf Happy Happy Playground | ringedge | title, attract and coins verified; gameplay, sound, payout not verified | not in the test |  |
| `puyo-puyo-quest-arcade` | Puyo Puyo!! Quest Arcade | ringedge | work in progress: does not start yet | not in the test |  |
| `transformers-shadows-rising` | Transformers: Shadows Rising | segashell | works (user: 100%), test menu (user) | pass | 2026-11-02 |
| `revolt` | Re-Volt (Tsunami cabinet) | tsunami | works (user: 100%): the cabinet's shell `Tsunami\TVTSHELL.EXE` (dump `games/arcadepc/revolt`), its console then `ReVolt.exe -coinop`; coin then the gas pedal starts a race; wheel, pedals and cabinet buttons through the TsuInput object (GetJoyInfo) | not in the test |  |
| `storm-racer-g` | Storm Racer G | wahlap | works (user: 100%) | pass | 2026-11-01 |
| `storm-rider` | Storm Rider | wahlap | works (user: 100%) | pass | 2026-11-01 |
| `ford-racing-full-blown` | Ford Racing Full Blown | europar | works (user: 100%) | pass | 2026-11-01 |
| `sega-rally-3` | SEGA Rally 3 | europar | works (user: 100%) | pass | 2026-11-01 |
| `showdown` | Showdown | segashell | works (user: 100%) | pass | 2026-11-01 |
| `grid` | GRID | europar | works (user: 100%) | pass | 2026-11-01 |
| `nobiteke-byon-byon` | Nobiteke! Byon Byon Daisakusen | konami | works (user: 100%) | not in the test |  |
| `gashaaaan` | Gashaaaan | konami | works (headless test) | fail: audio | 2026-11-02 |
| `gashaaaan-refill` | Gashaaaan Refill! | konami | works (user: 100%) | pass | 2026-11-01 |
| `otomedius` | Otomedius | konami | works (user: 100%) | pass | 2026-11-01 |
| `winning-eleven-2008` | Winning Eleven 2008 | konami | works (user: 100%), e-Amusement cards (loader's server) | pass | 2026-11-01 |
| `gti-club-supermini-festa` | GTI Club: Supermini Festa! | konami | works (user: 100%) | pass | 2026-11-02 |
| `winning-eleven-2012` | Winning Eleven 2012 | konami | works (user: 100%); e-Amusement cards (login, new player, match; save at a game's end not verified yet) | pass | 2026-11-01 |
| `bishi-bashi-channel` | Bishi Bashi Channel | konami | works (headless: attract, a 1P game) | pass | 2026-11-01 |
| `the-bishi-bashi` | The Bishi Bashi | konami | works (user: 100%) | pass | 2026-11-01 |
| `dance-dance-revolution-2013` | Dance Dance Revolution 2013 | konami | works (user: 100%), e-Amusement cards (loader's server) | pass | 2026-11-01 |
| `yugioh-duel-terminal-6` | Yu-Gi-Oh! 5D's Duel Terminal 6 | konami | works (user: 100%) | pass | 2026-11-01 |
| `hello-popn-music` | Hello! Pop'n Music | konami | works (user: 100%), e-Amusement cards for both players (loader's server) | pass | 2026-11-01 |
| `metal-gear-arcade` | Metal Gear Arcade | konami | works (user: 100%), gun, e-Amusement cards (loader's server), test menu | fail: audio | 2026-11-02 |
| `steel-chronicle-victroopers` | Steel Chronicle Victroopers | konami | works (user: 100%), two flight sticks, touch screen (mouse or gun), e-Amusement cards (loader's server) | pass | 2026-11-01 |
| `silent-scope-bone-eater` | Silent Scope: Bone Eater (Widescreen Edition) | konami | works (user: 100%), gun or mouse, scope tap / hold | pass | 2026-11-01 |
| `road-fighters-3d` | Road Fighters 3D | konami | works (headless tests: attract, race, test menu) | pass | 2026-11-01 |
| `contra-evolution` | Contra: Evolution | konami | works (user: 100%) | pass | 2026-11-01 |
| `wartran-troopers` | Wartran Troopers | konami | works (user: 100%) | pass | 2026-11-01 |
| `lethal-enforcers-3` | Lethal Enforcers 3 | konami | works (user: 100%), 1 gun, player 1's screen (rival's screen option), test menu | pass | 2026-11-01 |
| `lethal-enforcers-3-2p` | Lethal Enforcers 3 (2 players) | konami | works (user: 100%), 2 guns, 2 screens side by side, test menu | pass | 2026-11-01 |
| `coopers-9` | Cooper's 9 | konami | works (user: 100%), 2 guns, test menu | pass | 2026-11-01 |
| `rainbow-bombergirl` | Rainbow Bomber Girl | konami | works (user: 100%) | pass | 2026-11-01 |
| `castlevania-the-arcade` | Castlevania: The Arcade | konami | works (user: 100%), Little Witch unlocked; attract movie black | pass | 2026-11-01 |
| `silent-hill-the-arcade` | Silent Hill: The Arcade | konami | works (user: 100%) | pass | 2026-11-01 |
| `action-deka` | Action Deka | konami | works (user: 100%) | pass | 2026-11-01 |
| `hyper-sports-2011-remake` | Hyper Sports 2011 Remake | konami | works (user: 100%) | pass | 2026-11-01 |
| `thrill-drive-4` | Thrill Drive 4: Crazy Streets | konami | works (user: 100%) | pass | 2026-11-01 |
| `chase-chase-jokers` | Chase Chase Jokers | konami | works (user: 100%) | pass | 2026-11-01 |
| `galaxy-rangers` | Galaxy Rangers | arcadepc | works (headless tests: coins, start, level select, aim and shots, settings menu), 4 guns | pass | 2026-11-01 |
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
| `pigs-might-fly` | Pigs Might Fly | arcadepc | user 100% (2026-11-06) | not in the test |  |
| `space-warp-66` | Space Warp 66 | arcadepc | user 100% (2026-11-06), 1 spinner | not in the test |  |
| `wasteland-racers-2071` | Wasteland Racers 2071 | arcadepc | user 100% (2026-11-05) | not in the test |  |
| `polar-wars` | Polar Wars | arcadepc | works (user: 100%), 2 guns, last stage unlocked | not in the test |  |
| `skull-of-shadow` | Skull of Shadow | arcadepc | user 100% (2026-10-26), 4 guns | not in the test |  |
| `dino-invasion` | Dino Invasion | unis | user 100% (2026-10-26), 4 guns | pass | 2026-11-01 |
| `after-dark` | After Dark | unis | user 100% (2026-11-03), 2 guns | not in the test |  |
| `after-dark-2-3` | After Dark (2.3) | unis | user ok (2026-11-03), 2 guns; operator menu items not selectable | not in the test |  |
| `night-hunter-after-dark-chapter-ii` | Night Hunter: After Dark Chapter II | unis | user 100% (2026-11-03), 2 guns | not in the test |  |
| `night-hunter-after-dark-chapter-ii-3-0-2` | Night Hunter: After Dark Chapter II (3.0.2) | unis | user 100% (2026-11-03), 2 guns, 4 missions selectable (Lost Trails) | not in the test |  |
| `raccoon-rampage` | Raccoon Rampage | unis | user 100% (2026-11-05), 4 guns | not in the test |  |
| `crazy-ride` | Crazy Ride | unis | user 100% (2026-11-05), wheel, force feedback | not in the test |  |
| `crazy-speed-arcade` | Crazy Speed Arcade | unis | user 100% (2026-11-05), wheel | not in the test |  |
| `elevator-action-invasion` | Elevator Action Invasion | unis | user 100% (2026-11-05), 2 guns, virtual elevator doors and bezel, every level unlocked | not in the test |  |
| `wicked-tuna` | Wicked Tuna | unis | user 100% (2026-11-05), 4 rods on light guns or pads | not in the test |  |
| `fantasy-soccer` | Fantasy Soccer | unis | user 100% (2026-11-05), 4 players | not in the test |  |
| `mighty-moto` | Mighty Moto | unis | user 100% (2026-11-05) | not in the test |  |
| `big-foot-crush` | Big Foot Crush | unis | user 100% (2026-11-06), portrait, wheel; attract video not played (no Windows video backend in the build) | not in the test |  |
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
