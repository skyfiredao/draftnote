package main

import (
	"embed"

	"github.com/wailsapp/wails/v2"
	"github.com/wailsapp/wails/v2/pkg/options"
	"github.com/wailsapp/wails/v2/pkg/options/assetserver"
)

//go:embed all:frontend/dist
var assets embed.FS

func main() {
	bridge := NewBridge()

	err := wails.Run(&options.App{
		Title:                    "DraftNote",
		Width:                    1024,
		Height:                   720,
		EnableDefaultContextMenu: true,
		AssetServer: &assetserver.Options{
			Assets: assets,
		},
		OnStartup: bridge.startup,
		Bind: []interface{}{
			bridge,
		},
	})
	if err != nil {
		println("error:", err.Error())
	}
}
