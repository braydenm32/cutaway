#!/bin/sh
# Pack the plugin folder into Cutaway.streamDeckPlugin (a zip of the
# folder), which the app downloads and the release carry. Run it after
# changing anything in app.cutaway.streamdeck.sdPlugin; a test checks the
# two match.
set -e
cd "$(dirname "$0")"
rm -f Cutaway.streamDeckPlugin
zip -X -r -q Cutaway.streamDeckPlugin app.cutaway.streamdeck.sdPlugin -x '*.DS_Store'
echo "Packed Cutaway.streamDeckPlugin"
