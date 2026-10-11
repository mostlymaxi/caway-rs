# caway-rs

caway-rs is a Rust rewrite of [caway](https://github.com/PROxZIMA/caway), a Waybar module for music. It shows the artist and title of the current track. While music plays, it shows an audio visualizer made from the characters `▁▂▃▄▅▆▇█`.

This version has the same function and output as the original. The goal is less resource use and fewer dependencies, because the module runs all the time.

- It is one binary with no Rust crate dependencies.
- The only runtime dependency is cava. playerctl and jq are not necessary.
- It runs cava only while music plays.

In one test while music played, the total CPU use decreased from 15% to 0.6%. The memory use decreased from 8 MB to 5 MB.

## Requirements

- Waybar
- [cava](https://github.com/karlstav/cava)
- A media player that supports MPRIS, for example Spotify, Firefox, or mpv
- A font that has the characters U+2581 to U+2588 (`▁▂▃▄▅▆▇█`)

## Install

### Nix

1. Add the flake to the inputs of your flake:

   ```nix
   caway.url = "github:mostlymaxi/caway-rs";
   caway.inputs.nixpkgs.follows = "nixpkgs";
   ```

2. Use the package in the `exec` field of the Waybar module:

   ```nix
   exec = lib.getExe inputs.caway.packages.${pkgs.stdenv.hostPlatform.system}.default;
   ```

The Nix package includes cava. You do not need to install cava separately.

### Cargo

1. Install cava with your package manager.
2. Install caway:

   ```sh
   cargo install --git https://github.com/mostlymaxi/caway-rs
   ```

## Waybar configuration

```jsonc
"custom/caway": {
    "format": "{icon}{text}",
    "format-icons": {
        "Playing": " ",
        "Paused": "󰏥 ",
        "Stopped": "󰓛 "
    },
    "escape": true,
    "tooltip": true,
    "return-type": "json",
    "exec": "caway",
    "hide-empty-text": true,
    "max-length": 35,
    "on-click": "playerctl play-pause",
    "on-scroll-up": "playerctl previous",
    "on-scroll-down": "playerctl next"
}
```

The icons in this example need a Nerd Font. The click and scroll actions use playerctl. They are optional.

## Usage

```
caway [-b <bars>] [-f <framerate>] [-e] [-h]
```

| Option | Description |
|---|---|
| `-b <bars>` | Number of bars in the visualizer. The default is 16. |
| `-f <framerate>` | Frame rate of the visualizer. The default is 30. |
| `-e` | Disable the visualizer. The module shows only the track. |
| `-h` | Show the help text. |

When a track starts, the module shows the artist and title for 2 seconds. Then it shows the visualizer. When the player pauses, the module shows the track again. When no player is open, the text is empty.

The module sets the Waybar CSS class to the status of the player: `Playing`, `Paused`, or `Stopped`.

## Credits

This project is based on [caway](https://github.com/PROxZIMA/caway) by PROxZIMA. The output format, the options, and the Waybar configuration come from the original script.

## License

MIT. See [LICENSE](LICENSE).
