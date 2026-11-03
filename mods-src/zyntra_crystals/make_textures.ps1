# Draws the mod's 16x16 textures (original pixel art) from the character grids below.
# Run from this folder: powershell -File make_textures.ps1
Add-Type -AssemblyName System.Drawing
$ErrorActionPreference = "Stop"

function Save-Grid($path, $grid, $palette) {
    $bmp = New-Object System.Drawing.Bitmap 16, 16, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    for ($y = 0; $y -lt 16; $y++) {
        for ($x = 0; $x -lt 16; $x++) {
            $c = $grid[$y][$x]
            $bmp.SetPixel($x, $y, $palette[[string]$c])
        }
    }
    $path = Join-Path $PSScriptRoot $path
    New-Item -ItemType Directory -Force (Split-Path $path) | Out-Null
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
}

$C = { param($r, $g, $b) [System.Drawing.Color]::FromArgb(255, $r, $g, $b) }
$pal = @{
    "." = [System.Drawing.Color]::FromArgb(0, 0, 0, 0)
    "0" = & $C 22 10 34      # outline
    "1" = & $C 52 22 82      # deep purple
    "2" = & $C 88 38 132     # purple
    "3" = & $C 134 64 190    # bright purple
    "4" = & $C 196 120 236   # lilac
    "5" = & $C 246 196 255   # pink highlight
    "6" = & $C 255 250 255   # white glint
    "7" = & $C 70 44 30      # handle dark
    "8" = & $C 120 80 50     # handle light
}

# Crystal block: faceted purple crystal with glints
$block = @(
    "1122233221122332",
    "1223443322234432",
    "2234554322345543",
    "2345665432456654",
    "2234554321345543",
    "1223443211234432",
    "1122332111123321",
    "2111221122111222",
    "3221112233211123",
    "4332223344322234",
    "5443334455433345",
    "6554445566544456",
    "5443334455433345",
    "4332223344322234",
    "3221112233211123",
    "2111221122111222"
)
Save-Grid "assets/zyntra_crystals/textures/block/crystal_block.png" $block $pal

# Shard: a single diagonal crystal
$shard = @(
    "................",
    "...........00...",
    "..........0560..",
    ".........05540..",
    "........054430..",
    ".......0544320..",
    "......05443200..",
    ".....0544320....",
    "....0544320.....",
    "...0443320......",
    "..033320........",
    "..02320.........",
    "..0220..........",
    "...00...........",
    "................",
    "................"
)
Save-Grid "assets/zyntra_crystals/textures/item/crystal_shard.png" $shard $pal

# Sword: crystal blade, dark handle
$sword = @(
    ".............000",
    "............0560",
    "...........05540",
    "..........05430.",
    ".........05430..",
    "........05430...",
    ".......05430....",
    "......05430.....",
    ".00..05430......",
    ".0300430........",
    "..03330.........",
    "..0870..........",
    ".0870.0.........",
    "0870............",
    "070.............",
    "00.............."
)
Save-Grid "assets/zyntra_crystals/textures/item/crystal_sword.png" ($sword | % { $_.PadRight(16, '.') }) $pal
"textures written"
