#!/usr/bin/env python3
"""Look at and type into a headless GNOME session, for testing Snagster against GNOME's own
portal dialogs. Uses Mutter's remote-desktop and screen-cast D-Bus interfaces (the ones GNOME's
own remote desktop uses), so no portal dialog is involved in watching.

    python3 gnome-helper.py shot:/tmp/a.png key:Tab key:Return click:960,540 sleep:2

Each argument is one step, done in order: shot:FILE saves a PNG of the screen, key:NAME presses
and releases a key (Tab, Return, space, Escape, Left, Right, Up, Down), click:X,Y clicks there,
sleep:S waits.
"""
import subprocess
import sys
import time

from gi.repository import Gio, GLib

KEYS = {"Tab": 0xFF09, "Return": 0xFF0D, "space": 0x20, "Escape": 0xFF1B, "Left": 0xFF51,
        "Up": 0xFF52, "Right": 0xFF53, "Down": 0xFF54, "ISO_Left_Tab": 0xFE20}
BTN_LEFT = 0x110
RD, SC = "org.gnome.Mutter.RemoteDesktop", "org.gnome.Mutter.ScreenCast"
bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)


def call(dest, path, iface, method, args=None, rtype=None):
    r = bus.call_sync(dest, path, iface, method, args, GLib.VariantType(rtype) if rtype else None,
                      Gio.DBusCallFlags.NONE, 20000, None)
    return r.unpack() if r is not None else None


def connector():
    state = call("org.gnome.Mutter.DisplayConfig", "/org/gnome/Mutter/DisplayConfig",
                 "org.gnome.Mutter.DisplayConfig", "GetCurrentState")
    return state[1][0][0][0]


rd_path = call(RD, "/org/gnome/Mutter/RemoteDesktop", RD, "CreateSession", None, "(o)")[0]
sid = call(RD, rd_path, "org.freedesktop.DBus.Properties", "Get",
           GLib.Variant("(ss)", (RD + ".Session", "SessionId")), "(v)")[0]
sc_path = call(SC, "/org/gnome/Mutter/ScreenCast", SC, "CreateSession",
               GLib.Variant("(a{sv})", ({"remote-desktop-session-id": GLib.Variant("s", sid)},)), "(o)")[0]
stream = call(SC, sc_path, SC + ".Session", "RecordMonitor",
              GLib.Variant("(sa{sv})", (connector(), {"cursor-mode": GLib.Variant("u", 1)})), "(o)")[0]

node = []
loop = GLib.MainLoop()


def added(_c, _s, _p, _i, _sig, params):
    node.append(params.unpack()[0])
    loop.quit()


bus.signal_subscribe(None, SC + ".Stream", "PipeWireStreamAdded", stream, None,
                     Gio.DBusSignalFlags.NONE, added)
call(RD, rd_path, RD + ".Session", "Start")
GLib.timeout_add_seconds(10, loop.quit)
loop.run()
if not node:
    sys.exit("gnome-helper: the screen cast never started")


def key(name):
    for down in (True, False):
        call(RD, rd_path, RD + ".Session", "NotifyKeyboardKeysym", GLib.Variant("(ub)", (KEYS[name], down)))
        time.sleep(0.05)


def click(x, y):
    call(RD, rd_path, RD + ".Session", "NotifyPointerMotionAbsolute", GLib.Variant("(sdd)", (stream, x, y)))
    time.sleep(0.1)
    for down in (True, False):
        call(RD, rd_path, RD + ".Session", "NotifyPointerButton", GLib.Variant("(ib)", (BTN_LEFT, down)))
        time.sleep(0.05)


def shot(path):
    subprocess.run(["timeout", "15", "gst-launch-1.0", "-q", "pipewiresrc", f"path={node[0]}",
                    "num-buffers=1", "!", "videoconvert", "!", "pngenc", "!", "filesink",
                    f"location={path}"], check=False)


for step in sys.argv[1:]:
    what, _, arg = step.partition(":")
    if what == "shot":
        shot(arg)
    elif what == "key":
        key(arg)
    elif what == "click":
        x, y = arg.split(",")
        click(float(x), float(y))
    elif what == "sleep":
        time.sleep(float(arg))
    time.sleep(0.3)
call(RD, rd_path, RD + ".Session", "Stop")
