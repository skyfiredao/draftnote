#!/usr/bin/env bash
pkill -9 -f draftnote 2>/dev/null
rm -rf /Applications/draftnote.app
rm -rf ~/Applications/draftnote.app
rm -rf ~/Library/WebKit/draftnote
rm -rf ~/Library/Caches/draftnote
rm -rf ~/Library/Application\ Support/draftnote
rm -rf ~/Library/Preferences/wails.localhost.plist
rm -f  ~/Library/Preferences/com.wails.draftnote.plist
/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister \
  -r -domain local -domain system -domain user

[[ ! -e ~/Downloads/draftnote-mac-arm64.zip ]] && exit 1

unzip ~/Downloads/draftnote-mac-arm64.zip -d ~/Downloads/
hdiutil attach ~/Downloads/draftnote-mac-arm64.dmg
cp -R /Volumes/DraftNote/draftnote.app /Applications/
hdiutil detach /Volumes/DraftNote
rm -f ~/Downloads/draftnote-mac-arm64.*
xattr -cr /Applications/draftnote.app
open /Applications/draftnote.app
