# Changelog

What changed for people using Snagster, per version.

## 0.10.7 — 2026-11-22

Studio:

- **Sound from one program**: Add sound > Program sound lists the programs that have sound open
  (on Windows the ones playing first) and puts one in the scene, with its own meter, volume and
  mute next to the microphone and the computer sound. A scene remembers the program by name, so
  its sound comes in whenever it runs and goes quiet when it closes. When the computer sound is
  on too, a note says the program is heard twice, with a button to mute the computer sound.
  Windows 10 version 2004 and later; Linux with PipeWire (the Flatpak needs one override, which
  the menu shows); macOS 13 and later, with the Screen Recording permission. Elsewhere the menu
  says why and everything else works as before. AI apps get `list_sound_programs` and
  `set_program_sound`. The Mac app now needs macOS 12.3 or later (it was 12.0), as
  ScreenCaptureKit, which this uses, came in 12.3.

For AI apps (every system):

- **Clearer answers when an AI app gets a tool's arguments wrong**: every call is checked
  against the tool's description first, and a misspelt argument, a number sent as text, a name
  that isn't one of the choices or a value out of range is refused with what was sent and what
  the tool takes (before, some were ignored or quietly changed to the nearest allowed value).
  Every argument now says what it is, its unit and its range.
- Ending a live stream and turning the replay buffer off are marked as ending something, so AI
  apps ask before they do it. The server tells AI apps its description, website and icon.
- **A free skill for AI coding agents** (Claude Code and others):
  [skills/snagster/SKILL.md](skills/snagster/SKILL.md) explains when and how to use Snagster's
  tools, with setup and recipes.
- **MCP bundles**: every release has a one-file MCP bundle (`.mcpb`) for Windows, Linux and
  macOS, which AI apps that take bundles install with one click, and a `server.json` for the
  official MCP Registry.

Using Snagster from the keyboard (every system):

- **F6 and Shift+F6 move between the ribbon, the tools, the picture, the options and Recent**;
  Tab moves from control to control, the arrow keys within a part, Enter or Space uses the
  control, Esc goes back to the picture. A ring shows which control has the keyboard, and
  screen readers follow every move. On the picture the arrow keys move the picked mark.
- **Windows: Alt or F10 shows key tips on the ribbon**, as in Office: type a tab's letter, then
  a tool's. Alt and a letter opens a tab at once.
- **Linux: hold Alt to see the ribbon tabs' letters**; Alt and a letter opens that tab (F10
  still opens the main menu).

Live streaming (every system):

- **A Studio sound that can't start is always reported**: on a busy computer a missing sound
  file or a microphone in use could be kept as a silent input without the note saying so.
  Snagster now waits for each sound to start or fail.
- **See trouble while you're live**: the Stream tab's Status says in a few words when the
  upload can't keep up, the computer can't encode fast enough, or a service keeps reconnecting,
  with what to do on hover (a bad one also shows in the status line and the capture panel). A
  warning shows once it has held for three seconds (a bad one at once) and stays ten seconds,
  so it doesn't flicker; the first ten seconds after going live are quiet.
- **Stream details** (the chart button next to the state): for each service the bitrate really
  sent against the one set, how much is waiting to be sent, frames dropped by the network,
  how long it has been connected and reconnects; for the computer the frames it dropped and
  the encoder's time per frame. Shares are over the last ten seconds.
- Frames thrown away when a reconnect starts fresh, or to make room in a full queue, now count
  as dropped too.
- For AI apps: **stream_stats** gives the same numbers and warnings.
- **Draw while you're live (Studio > Ink > Draw, Ctrl+Shift+F7; ⌥⇧⌘D on a Mac)**: the ink is
  in the stream and in Studio's video, on every source that shows that part of the screen. It
  fades a few seconds after the pen lifts (1 to 10 s, 3 s at first) or stays until **Clear**
  (Ctrl+Shift+F6; ⌥⇧⌘X). Pick the colour and pen width in the Ink group; Esc stops drawing,
  Ctrl+Z (⌘Z) takes back the last stroke, or right after Clear what it removed. On Windows you
  draw over every monitor (a thin border and a hint show, never captured; Snagster's own window
  stays usable); other programs' screen shares don't see the ink, only Studio's stream and video
  do. On macOS and Linux you draw on the Studio preview for now (Wayland doesn't let apps cover
  the screen; drawing over the screen on macOS and X11 is still to come). For AI apps:
  **ink_clear**, and the settings studio.ink_mode, studio.ink_fade_s, studio.ink_color and
  studio.ink_width.

Recording (every system):

- **Two recordings at once no longer confuse the recording bar**: when an AI app's
  record_screen or a Studio recording ended while your own recording ran, the bar went back to
  "waiting for the screen". Each recording now counts on its own.
- **Replay buffer: save a moment after it happened.** Record > Replay keeps the last 30, 60, 90
  or 120 seconds of the screen the mouse is on in memory (encoded on the graphics chip where it
  can, at most about 512 MB); **Save replay** (Ctrl+Shift+F8, ⌥⇧⌘B on a Mac, or the tray menu)
  saves them at once as an MP4 in the recordings folder, with the computer's sound on Windows.
  It is off until you turn it on; a "Replay 60 s" badge next to the tabs and a dot on the tray
  icon show while it is on. Settings > Recording > Replay sets the length and sound, and can
  turn it on when Snagster starts (on Wayland only when the shared screen is remembered, so no
  dialog pops up by itself). AI apps use replay_start, replay_save, replay_status and
  replay_stop.

Marking up pictures (every system):

- **Ctrl+= , Ctrl+- and Ctrl+0 zoom the picture** (⌘ on a Mac, also in the View menu), as in
  other editors. Before, they made the whole window's text and buttons bigger or smaller; for
  that, use Windows' Text size or your desktop's display scale.
- **Adjust a mark right after drawing it**: a new arrow, line, box, oval, highlighter box,
  spotlight or magnifier stays picked with handles: drag an end of an arrow, or a corner or
  side of a box, to reshape it without switching tools. With a drawing tool, pressing on a mark
  that tool made picks it up to move it; anywhere else draws a new one.
- **Shapes start exactly where you press**: a box, arrow or line used to start a few pixels on
  from the click. Alt draws a box or oval out from its centre.
- **A box highlighter** for marking text: drag a see-through yellow box over it. Freehand
  strokes are one click away in the tool's options.
- **Shift+Enter starts a new line in text**, as the hint said; Enter finishes.
- Copying right after drawing still copies the picture; marks are copied with the selection
  tool.
- **Show in folder** on a right-click of a picture in Recent and on the picture itself (a new
  capture shows its library copy). On Linux and macOS, Show in folder for videos and Library
  items opened the file itself; it now opens its folder with the file selected (on Linux
  through the desktop's portal, so also in the Flatpak).
- **KDE Plasma:** Snagster's dialogs, pins and recording bar show Snagster's icon and sit
  with Snagster in the taskbar (they showed another program's icon before).
- **Dialogs fit what they hold**: New image, Resize, Rename and the other small dialogs no
  longer keep empty space below their buttons (on every system), and on KDE Plasma they no
  longer offer Maximise.
- **Black and white colours show on every panel**: a colour swatch or quick style that would
  melt into the panel (black in dark mode, white or yellow in light mode) gets a thin edge.
- **The desktop is called "Desktop"** when you point at it in the Area overlay on Linux (Plasma
  showed "Desktop @ QRect(...)"), and it is no longer offered as a window to capture (nor is
  Windows' Program Manager).
- **Cancelling the desktop's screenshot dialog** says so plainly, and the screenshot portal's
  other answers are translated.
- **Cancelling screen sharing** in the desktop's dialog now says so (on KDE, where Cancel looks
  the same as a stop without a reason, the message names both).
- **A cancelled recording on Wayland no longer saves a black video**: it fell back to XWayland,
  which sees only a black screen, and said "Saved". It now stops and says it was cancelled, and
  a recording that never got a picture leaves no empty file behind.
- `snagster record`, `snagster capture` and AI apps over MCP tell the desktop they are
  Snagster, so KDE remembers one screen-share answer for all of Snagster.

Windows:

- **Snagster takes Windows' accent colour** (Settings > Personalization > Colors) for its
  buttons, selections and focus rings, as Windows 11's own apps do, and follows a change within
  a couple of seconds. A light accent is darkened until white text on it reads. Linux and macOS
  already followed theirs.
- **A Microsoft Store edition** is ready to submit: the Store signs it (no SmartScreen or
  "unknown publisher" warning) and keeps it up to date; a Snagster installed from the Store
  doesn't download updates itself. `PRIVACY.md` says what Snagster does and doesn't send.
- Builds can be signed with Azure Artifact Signing (`Snagster.exe` and the installer) once it
  is set up.

Linux:

- **X11: dialogs belong to Snagster's window**: Settings, the Library and every dialog stay
  above Snagster and keep out of the task bar, and the fixed-size ones (New image, Resize,
  Rename and the like) have no Minimise or Maximise button on KDE.
- **Wayland: dialogs open inside Snagster's window**: New image, Resize, Rename and the other
  fixed-size dialogs dim the picture behind them and close with Esc, Ctrl+W or their close button (a
  click beside them doesn't lose what you typed). Settings, the Library and the other big
  boxes fit inside the window. A box has the keys from when it opens until you click the
  editor (a click on the box gives them back to it), and meanwhile the editor's shortcuts
  leave the picture alone. Esc closes only the box on top.
- **Every screen in a Screen capture on Wayland**: the desktop's dialog lets you pick all your
  screens once, and Screen, Area and scrolling captures then see the whole desk, each screen
  where it sits (screens with different scaling are matched to the sharpest). Recording keeps
  its own remembered screen.
- **Areas start where you pressed**: an area began a few pixels on from the click (where the
  drag was noticed), so the cut was off on Wayland. Area on Wayland is Snagster's own overlay
  again: no desktop dialog after the first share.
- **A notification** says "Recording saved" or "GIF saved" when Snagster is in the tray or
  behind other windows.
- The keyboard-shortcut settings say **Super** (Linux) and ⌃ ⌥ ⇧ ⌘ (Mac) instead of Win.
- **The recording bar on Wayland**: the editor is minimized where it is and the bar is a small
  window of its own; after Stop the editor comes back at its old place and size. On KDE
  Plasma the bar opens in the bottom-right corner and stays above other windows, and the
  editor also comes back after a hotkey Stop. The desktop's own "stop sharing" (GNOME's top
  bar, KDE's tray) now stops and saves the recording too. The bar's red dot no longer blinks,
  its clock shows hours, and its texts are translated.
- Cancelling the desktop's share dialog now says plainly that nothing was shared, and
  `snagster record` works in the Flatpak on Wayland.

Settings, the Library and the tray (every system):

- **A shorter tray menu**: All-in-one, Area, Window, Screen, Scrolling, Grab text and Record
  video stay at the top; the other captures are under More captures.
- **The first time you close the window, Snagster says it keeps running** in the corner of the
  taskbar (the tray on Linux, the menu bar on a Mac when you chose that), how to exit, and offers
  Keep running or Always exit. Before, the only word of it went to the status line of the window
  that had just closed.
- **More of Snagster speaks your language**: the Keyboard shortcuts window, the hotkey names in
  Settings, messages with numbers or names in them (interval and timed captures, presets, GIF
  frames, tags), the tray menu, tooltips, the capture panel's status line and the arrow
  heads' None / End / Both. Sizes read "1200 × 800", and "Couldn't make the picture" now says
  it would be too big.
- **The window's title names the open picture** ("Area 3 – Snagster"; on a Mac just the
  picture's name), so the taskbar, Alt+Tab and the Dock say which one you're on. A picture
  saved with Save as under a new name takes that name, in Recent too.
- **Settings opens on the pane you used last**, also after Snagster restarts.
- **The Library picks a capture with a click** (a blue ring shows it); Delete moves it to the
  Recycle Bin or Trash, with the usual Undo. In the search box Delete still edits the text.
- **Stop recording in the tray's menu**: while a recording runs it is the first item.
- **Library tiles with bigger text**: at Windows' Text size of 150 % and more the tiles grow and
  their lines no longer run into each other; a title too long for its tile ends in "…" and is
  shown whole in the tooltip. The Library's sort and search texts are now translated.
- **The Welcome screen fits any window**: its cards go two by two, or one under the other, in a
  narrow window or with big text, and a short window scrolls with the cards first. Its texts are
  translated and the Capture card shows your own All-in-one key.
- **The ribbon's "More" means one thing**: the groups that don't fit a narrow window. The tabs
  that don't fit are behind a ▾ at the end of the tab row, and every tab's last group is now
  called Commands.

macOS:

- **Quit & Reopen after allowing Screen Recording**: once System Settings is open from
  Snagster's window, the window offers to start Snagster again, which macOS needs before the
  permission counts.
- **Settings has a toolbar** of seven icons (General, Capture, Recording, Editor, Hotkeys,
  Streaming, AI Apps), as Mac apps' Settings windows do; the window is named after the one
  chosen.
- **Capture keys that leave the Mac's own alone**: All-in-One ⌥⇧⌘A, Window ⌥⇧⌘W, Screen ⇧⌘F,
  and Pin to Screen ⌥⌘P (⇧⌘A, ⇧⌘W and ⇧⌘P are the Mac's Deselect All, Close File and Page
  Setup). Windows and Linux keep Ctrl+Shift+A, W, F and P.
- **The Edit menu greys out** Undo, Redo, Cut, Copy and Select All when there is nothing for
  them to do, and right after a delete Undo reads "Undo Move to Trash".
- **View > Hide Ribbon / Show Ribbon** (⌥⌘T), and **File > Move to Trash on ⌘⌫** for a saved
  picture (in the Library ⌘⌫ moves the capture picked there).
- **The menu bar icon turns into a stop square** while a recording runs.

Fewer crashes on odd input (found by new tests that feed Snagster broken files every night):

- **A callout with very small text no longer crashes Snagster** (pressing Smaller until it
  stops, with the tail to the side, did).
- **Broken files are errors, not crashes or hangs**: a project whose marks sit absurdly far off
  the picture or have nonsense sizes opens (those marks aren't drawn); a broken MP4 no longer
  makes video details, or opening it on Linux and macOS, wait for ever or crash; a tiny broken
  TIFF no longer takes a quarter of a gigabyte of memory.
- **Long step-by-step guides are quick**: 250 steps took over 30 seconds as one picture; a guide
  too long for one picture now says to save it as a PDF or web page.
- For AI apps: combine_images and make_guide refuse sizes that wouldn't fit in memory, and say
  so; editor_apply's cut_out far outside the picture is simply nothing to cut.
- Updates: a SHA256SUMS line written as `<sum> * name` now matches its download.

Linux and macOS:

- **Videos start smoothly**: the time the encoder took to start was counted as recorded, so a
  video began with its first picture held for that long and a time limit ended it early. The
  clock now starts at the first picture, as on Windows.

## 0.10.6 — 2026-11-17

Linux with Wayland (checked on Ubuntu 26.04 with GNOME):

- **Asked once**: the desktop asks to share the screen the first time and remembers it, so
  recording, Studio, live streaming and screen captures start without the dialog after that.
  Capturing a whole screen no longer opens the desktop's screenshot tool.
- **Hotkeys work on Wayland**: Snagster gives its hotkeys to the desktop (GNOME 48 and later,
  KDE Plasma 6), which asks you once and lets you change them in its keyboard settings. Where a
  desktop can't, a shortcut you set there can run `snagster --action Area` (or AllInOne, Window,
  Screen, RecordGif…).
- Snagster follows the desktop's accent colour, and GNOME and KDE get a main menu at the end of
  the tab row (F10) with Settings, Keyboard shortcuts, Getting started, About and Quit.
- Fixed: Studio and recording showed nothing in the Flatpak; the first click on the recording
  bar's Stop was lost; Space in "Choose the area to record" took a picture instead of recording
  the whole screen (Windows too); Record GIF stopped after a few frames.

AI apps:

- **Every option in Snagster can be reached over MCP**: every button, menu item and setting has a
  tool or a typed setting (73 tools), and `stream_start` sets up a whole stream in one call.


Snagster follows each system's own design rules more closely (Windows 11, GNOME, KDE Plasma,
macOS):

- **Keys do what the menus say**: Ctrl+S saves (it asks where only the first time), Ctrl+Shift+S
  is Save as, Ctrl+Alt+S saves to the save folder at once, Ctrl+P prints, Ctrl+Shift+P pins the
  picture, and Ctrl+N makes a new picture. On Linux and macOS, Ctrl+Q / ⌘Q quits. Windows such
  as Settings close with Ctrl+W (⌘W) as well as Esc.
- **On a Mac** shortcuts are shown as ⌘, ⌥, ⇧ and ⌃, and new installs get capture hotkeys a Mac
  keyboard has (⇧⌘2 for All-in-one) that leave the system's ⇧⌘3, 4 and 5 alone. The screen
  permission message names "Screen & System Audio Recording" and says to choose Quit & Reopen.
- **Deleting from the Library** moves the capture to the Recycle Bin (Trash on Linux and macOS),
  so it can be brought back; it used to be deleted for good.
- **Dialogs** put their buttons in the system's order (on GNOME and macOS Cancel comes first and
  the action last). Return runs the action and Esc cancels; "Go live" is never started by Return.
- **Linux**: GNOME on Wayland gets a proper Adwaita-style title bar. Closing the window really
  closes it on desktops without a tray (stock GNOME), new installs don't keep Snagster in the
  tray unless you choose it, and Linux and macOS say Trash, Quit and tray instead of Windows words.
- **Screen readers** (Narrator, Orca, VoiceOver) can now read Snagster's buttons.
- Ribbon labels are a little bigger and easier to read, the window title bar stays dark when
  the system is in light mode, and menu commands that ask for more use the "…" character.

Windows:

- **Smoother on company laptops and in remote sessions**: the window is now drawn with
  Direct3D 12 instead of OpenGL. Company laptops often have old OpenGL drivers, and Remote
  Desktop, Citrix and other VDI sessions offer only OpenGL 1.1 (where Snagster used to lag or
  not start at all); Direct3D 12 is part of Windows and uses Microsoft's own software renderer
  when there is no graphics chip. OpenGL is still used when Direct3D can't start. If drawing
  lags, crash.log (Help > Problem reports) now says so, with the graphics chip and whether it
  was a remote session, and `snagster --hw-report` lists the graphics chips.
- **An installer (MSIX)**, made by every build: it installs to Start and Settings > Apps,
  updates through App Installer, starts with Windows through its startup task and opens
  `.snag` projects. It joins the downloads once it is signed.

## 0.10.5 — 2026-11-17

Fixed on Linux with Wayland (found and checked on Ubuntu 26.04 with GNOME):

- **No more screenshot loop**: Studio's screen preview asked the desktop for a screenshot every
  frame, which flooded GNOME until the computer had to be restarted. Studio, recording and live
  streaming now use the desktop's screen sharing (one "Share" dialog), and Snagster never asks
  for more than a few screenshots in a row.
- **Capture no longer seems frozen**: Snagster minimized itself before a capture and couldn't come
  back (only you can un-minimize a window on Wayland). It now stays put, and a capture opens the
  desktop's own screenshot tool (area, window or screen); the picture goes straight to the
  editor. This also fixes the Flatpak's "no access to the desktop".
- **Recording**: the video starts when you've clicked Share (the bar says "Waiting for screen
  sharing" until then), the recording bar keeps its small size and can be dragged out of the
  way, and the window gets its size back afterwards.
- **Scrolling capture** says it isn't available on Wayland yet instead of starting; circle,
  freehand and polygon captures keep their shape.
- On Linux and macOS the start-up setting reads "Start Snagster when you log in".

## 0.10.4 — 2026-11-16

New:

- **Flatpak download for Linux**: every release now has `Snagster-linux-x86_64.flatpak`, the
  sandboxed Snagster for any Linux distribution. Install it with
  `flatpak install --user Snagster-linux-x86_64.flatpak`. Updates come with each new download.

## 0.10.3 — 2026-11-15

Fixed:

- **Print Screen while Snagster is in the tray**: the capture opened full screen but stayed
  invisible, so the screen seemed frozen. It now shows, and cancelling puts Snagster back in the
  tray (or minimized) where it was.
- **"Couldn't move the old version aside: Access is denied (os error 5)"** when updating: an
  update started from a copy still running under its old name now works.
- **The Linux download runs on Ubuntu 24.04, Debian 13 and other current systems**: 0.10.1 and
  0.10.2 were built on Ubuntu 26.04 and wouldn't start on systems older than that. Linux is now
  built on Ubuntu 24.04 (needs glibc 2.39 or newer), checked in every build.

## 0.10.2 — 2026-11-15

New:

- **Linux: works on Wayland and inside a Flatpak**: recording and live streaming share the
  screen through the desktop's own screen-sharing dialog (you pick the screen), and still
  captures use the desktop's screenshot service when the screens can't be read directly.
- **Linux menu entry and store listing**: a desktop entry, AppStream information and icon
  under the name `io.github.SoloSaftis.Snagster`, checked in every build.
- **Updates in a Flatpak** come from Flatpak (your software center); Snagster doesn't download
  them itself there.
- `AI-DISCLOSURE.md` lists which parts of Snagster are AI-generated and to what extent.

## 0.10.1 — 2026-11-15

New:

- **Recent shows what you took before**: the strip of recent captures now also lists the
  pictures and videos already on your computer (library, save folder, videos folder), newest
  first, not only this session's. Click one to open it.
- **Capture a window from a list**: the Window button opens a list of the open windows to pick
  from (or click one on the screen, as before).
- **One Snagster at a time**: opening Snagster again (Start menu, Dock, a file you double-click)
  brings the running one to the front instead of starting another; a file goes to it. Switch on
  "Allow more than one Snagster at a time" in Settings if you want more.
- **Settings and dialogs in their own windows**: Settings, the Library, GIF recordings, the
  timer, the language list and every dialog open as separate windows, centered on Snagster's.
  Snagster itself opens in the middle of the screen.
- **Choose the graphics card for encoding**: with two graphics cards of the same maker, each
  card's encoder gets the card's name in the encoder list (Windows); on Linux each graphics chip
  that can encode (Intel, AMD, NVIDIA) is listed on its own.
## 0.10.0 — 2026-11-15

New:

- **Studio is the live workspace**: **Go live** sits next to **Record** in Studio, and the Stream
  tab becomes the control surface while you're on air: a button for each scene (Ctrl+1 to Ctrl+9
  switch scenes) and the current scene's sounds as buttons with level meters (click to mute).
- **Sound per scene, up to 6 sources each**: every scene has its own sound (computer sound, any
  microphone, or a WAV sound file such as intro music, played over and over), plus sounds kept
  in every scene (your microphone, set up once). Switching scenes fades the sound across instead
  of cutting. Sound files work on every system; computer sound and microphones are Windows-only
  for now. Your old Studio sound settings move over by themselves.
- **One-command hardware report**: `snagster --hw-report` tests the graphics chips, every video
  encoder, recording and streaming on this computer and writes it all to
  `snagster-hw-report.txt` (nothing is sent anywhere). The GitHub "Hardware report" form asks for it.
- **Linux laptops with two graphics chips** (say Intel plus NVIDIA): Snagster now tries each chip
  and encodes on the first one that can, instead of giving up on the first one it finds.
  `SNAGSTER_VAAPI_DEVICE=/dev/dri/renderD129` picks one by hand.
- **Linux recording is faster and works on more desktops**: it uses the same fast screen copy
  as streaming (X11 shared memory), and where the Wayland screen copy isn't available (for
  example WSLg) it records through XWayland instead of failing. macOS recording uses the
  CoreGraphics copy that streaming uses.
- **No crash on some Wayland desktops**: the screen-capture library could close Snagster when
  the desktop lacks a screen-copy feature; now Snagster says it can't see the screen.
- **NVIDIA graphics chips on Linux**: recording and streaming now use NVIDIA's own encoder
  (NVENC) on Linux, which also brings HEVC (H.265) to Linux. Before, Linux computers with an
  NVIDIA card encoded on the processor. Works with any NVIDIA driver from 2018 on; computers
  without one are not affected. Not yet tried on a real NVIDIA card.
- **Choose the video encoder**: Settings > Recording or Streaming > "Video encoder" lists the
  encoders this computer has (Intel Quick Sync, NVIDIA, AMD, Apple's media engine, VA-API,
  software). *Automatic* stays the default. Choosing one that can't start never stops a
  recording or stream: Snagster uses the best one that works and says so.
- **A stream survives its encoder failing**: when the graphics chip's encoder stops working in
  the middle of a stream (a driver reset, a laptop switching chips), another encoder takes over
  within a frame and the stream goes on, instead of ending.
- **More responsive**: exporting several pictures, Word/PDF/web guides, combining pictures,
  templates, opening and importing files, Print/Email/"Open in another program" and the
  Recycle Bin now work in the background, so the window never stops answering ("Not
  responding") while they run. The Share tab no longer checks the cloud folders on every frame
  (a disconnected network drive could freeze it), "New from template" no longer redraws every
  picture while the menu is open, and "Start when Windows starts" no longer runs `reg.exe`.
- **Multistreaming**: go live on several services at once (say Twitch and YouTube) from one
  encode. Settings > Streaming > "Also stream to" takes up to four more services, each with its
  own server and stream key (kept as safely as the main key). The picture is sent at the size,
  frame rate and bitrate every chosen service takes. Each service connects, reconnects and drops
  frames on its own, so one slow or failing service doesn't affect the others, and the Stream tab
  shows how each one is doing. AI apps can multistream too (`stream_start` with `also`).
- **Live streaming** to Twitch, YouTube, Kick, Facebook Live, Restream or any RTMP/RTMPS server:
  the whole screen, a window (followed when it moves), an area or the Studio scene. A Stream tab
  in the ribbon and in the capture panel, a red LIVE badge with bitrate, dropped frames and
  time on the air, and Settings > Streaming (server, picture size, frame rate, bitrate,
  keyframes, graphics-chip encoding, sound, reconnecting). Snagster asks before going live the
  first time each run, and Stop is instant. It reconnects by itself when the connection drops.
  Optionally saves a copy of exactly what was streamed (FLV) in the recordings folder. Sound is
  streamed on Windows only for now. Not yet tried against the real services.
- **4K and 120 fps streams**: YouTube up to 3840 × 2160 at 60 fps (with YouTube's recommended
  bitrates, and HEVC by itself at 4K where the computer can encode it); custom servers up to 4K
  at 60 fps or 1440p at 120 fps. Settings > Streaming only offers what the service takes.
- **Streaming uses far less processor time** on Windows with a graphics-chip encoder and on
  macOS: the screen's pictures stay on the graphics chip from the copy to the encoder
  (about 4 % of one core instead of 114 % at 1080p30 on an Intel laptop, 18 % instead of 212 %
  at 1080p120). Other computers copy as before.
- **The stream key stays private**: encrypted for your Windows account, in the macOS Keychain,
  or in a file only you can read on Linux. It is never in the settings file and never shown in
  messages.
- **Snapping while you select**: the window, button, list or panel under the pointer is
  highlighted; click to take it, turn the mouse wheel to step out to the bigger part around it,
  hold Ctrl for the whole window. Windows reads controls with UI Automation; on a Mac allow
  Snagster under Privacy & Security > Accessibility.
- **Scrolling capture scrolls by itself**: it goes back to the top, scrolls, finds where the
  new part starts (sticky headers and footers kept once) and stops at the end. Where Snagster
  can't scroll other programs (Wayland, a Mac without the permission, Linux without xdotool),
  you scroll and press Stop. New settings: wheel notches per step, longest picture, start from
  the top.
- **Graphics-chip video encoding on Mac and Linux** (VideoToolbox, VA-API), with OpenH264 as the
  fallback; faster OpenH264 on several processor cores. Not yet tried on real Mac or Linux
  hardware.
- **Snagster's own AAC sound encoder**: the fallback when the system has none, and the
  groundwork for streams with sound on Linux once sound capture arrives there.
- **For AI apps**: 66 MCP tools (was 51 in 0.9.0), including going live, stream status and
  stopping; `update_mark`, `run_preset`, `simplify`, `quick_save`, `capture_last_area`, Studio
  scene tools, `library_tag` and `library_delete`. Long calls report progress; results come
  with structured data; resource templates and completions.
- **Snagster.exe has its own icon** in Explorer, Start, the taskbar and Alt+Tab.

Fixed:

- The window no longer freezes ("Not responding") while saving, scanning the library, opening
  GIFs, reading the clipboard or listing windows; that work happens in the background.
- Saving a very large still GIF is about 14 times faster (23 s to 1.7 s for 26 megapixels).
- Recording on a computer without speakers records the video without sound instead of failing.
- Redo while typing text could change the wrong mark.
- Smart redact labelled IP addresses as phone numbers.
- Norwegian shows as "Norsk bokmål" in the language list.
- AI apps: opening a new picture kept the old picture's redo steps; a negative number acted on
  the first mark, source or scene.
- Broken or hostile input (project files claiming huge pictures, damaged settings files, odd
  network data from a streaming server or an AI app) can no longer crash Snagster or make it
  reserve huge amounts of memory.
- The HTTP server for AI apps could stop accepting connections after many failed ones.

## 0.9.0 — 2026-11-14

- A Word-style ribbon (Home, Capture, Record, Studio, Edit, Image, Share, View, Help) with icons
  and a tooltip on every tool.
- 86 interface languages, picked from your computer, right to left for Arabic, Persian, Hebrew,
  Urdu and Uyghur.
- Videos with sound on Windows: the computer's sound, a microphone or both.
- Studio mode: scenes of screens, windows, areas, pictures, text and colours, with a mixer and
  recording.
- Automatic updates from GitHub (daily check, can be turned off).
- Pictures in Pictures/Snagster, videos in Videos/Snagster.
- An MCP server any AI app can use (51 tools), over stdio or HTTP, including local AI.
- A failing frame no longer closes the window; it is logged to crash.log.

## 0.8.0 — 2026-11-13

- Video recording to MP4 (H.264 or HEVC) or WMV at 1 to 120 frames per second, encoded by the
  graphics chip on Windows (Intel, AMD, NVIDIA), OpenH264 on Linux and macOS. Record menu,
  capture panel Video tab, Settings > Recording, pause and resume.

## 0.7.0 — 2026-11-13

- An MCP server for AI apps (22 tools) and command-line commands (capture, ocr, redact, effect,
  convert, call). Settings > AI apps. Downloads for Windows, macOS and Linux. MIT or
  Apache-2.0.

## 0.6.0 — 2026-11-13

- First release of Snagster (formerly Snag): capture areas, windows, menus, screens and shapes,
  scrolling capture, markup (arrows, boxes, text, callouts, steps, emojis, blur, spotlight,
  magnifier), effects, text recognition and smart redact on Windows, GIF and step recording,
  presets and hotkeys, tray icon and capture panel, the library, `.snag` projects, and
  PNG/JPEG/BMP/GIF/WebP/TIFF/PDF/SVG/Word output.
