# dmgbuild settings for Asiri's disk image. tools/release/make-dmg.sh passes every value with
# -D, so the window is laid out in one place (that script); this file only maps it onto
# dmgbuild's options. dmgbuild runs it with `defines` set.
import os.path


def pair(text):
    x, y = text.split(",")
    return int(x), int(y)


app = defines["app"]  # noqa: F821 (set by dmgbuild)
name = os.path.basename(app)
width, height = (int(v) for v in defines["window"].split("x"))  # noqa: F821 (title bar included)

# LZMA makes the smallest download (updates come over Tor); it opens on macOS 10.15 or later.
format = "ULMO"
filesystem = "HFS+"
files = [app]
symlinks = {"Applications": "/Applications"}
hide_extensions = [name]
icon = defines["icon"]  # noqa: F821
# background@2x.png beside it is combined in, so the picture is sharp on Retina screens.
background = defines["background"]  # noqa: F821

# A bare window: no toolbar, sidebar, path or status bar, the icons where the picture expects.
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
window_rect = ((200, 140), (width, height))
default_view = "icon-view"
include_icon_view_settings = True
include_list_view_settings = False
arrange_by = None
show_icon_preview = False
show_item_info = False
label_pos = "bottom"
icon_size = float(defines["icon_size"])  # noqa: F821
text_size = float(defines["text_size"])  # noqa: F821
icon_locations = {
    name: pair(defines["app_at"]),  # noqa: F821
    "Applications": pair(defines["applications_at"]),  # noqa: F821
}
