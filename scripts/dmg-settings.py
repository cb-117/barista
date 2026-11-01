# dmgbuild settings for the Barista installer. Used by scripts/make-dmg.sh.
# The background (Support/dmg-background.tiff, from scripts/dmg-background.swift) is drawn
# for this exact window size and these icon positions.
import os.path

app = defines['app']
app_name = os.path.basename(app)

format = 'ULFO'                  # lzfse-compressed, read-only; macOS 10.11+
filesystem = 'HFS+'
size = None                      # fit to contents
files = [app]
symlinks = {'Applications': '/Applications'}
icon = defines['volume_icon']
badge_icon = None

background = defines['background']
window_rect = ((200, 140), (660, 420))
default_view = 'icon-view'
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
include_icon_view_settings = True
icon_size = 112
text_size = 13
arrange_by = None
icon_locations = {
    app_name: (170, 190),
    'Applications': (490, 190),
}
hide_extensions = [app_name]
