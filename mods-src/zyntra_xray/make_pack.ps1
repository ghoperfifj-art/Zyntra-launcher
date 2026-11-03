# Generates the Zyntra Xray resource pack: natural rock and soil become empty models
# (invisible), ores draw every face (no cullface), so they show through the invisible rock.
# Run from anywhere: powershell -File make_pack.ps1
$ErrorActionPreference = "Stop"
$root = $PSScriptRoot
$utf8 = New-Object System.Text.UTF8Encoding $false
function Write-Json($rel, $text) {
    $path = Join-Path $root $rel
    New-Item -ItemType Directory -Force (Split-Path $path) | Out-Null
    [IO.File]::WriteAllText($path, $text, $utf8)
}
Get-ChildItem $root -Directory | Remove-Item -Recurse -Force

# Old fields for Java 1.20.x-1.21.8, new ones for 1.21.9+ and Horizon 26.3 — consistent with each other.
Write-Json "pack.mcmeta" '{"pack":{"description":"Zyntra Xray — руды сквозь камень","pack_format":15,"supported_formats":[15,999],"min_format":15,"max_format":999}}'

# Shared models
Write-Json "assets/zyntra_xray/models/block/hidden.json" '{"textures":{"particle":"minecraft:block/stone"},"elements":[]}'
$faces = '"down":{"texture":"#all"},"up":{"texture":"#all"},"north":{"texture":"#all"},"south":{"texture":"#all"},"west":{"texture":"#all"},"east":{"texture":"#all"}'
Write-Json "assets/zyntra_xray/models/block/full_nocull.json" ('{"textures":{"particle":"#all"},"elements":[{"from":[0,0,0],"to":[16,16,16],"faces":{' + $faces + '}}]}')
$colFaces = '"down":{"texture":"#end"},"up":{"texture":"#end"},"north":{"texture":"#side"},"south":{"texture":"#side"},"west":{"texture":"#side"},"east":{"texture":"#side"}'
Write-Json "assets/zyntra_xray/models/block/column_nocull.json" ('{"textures":{"particle":"#side"},"elements":[{"from":[0,0,0],"to":[16,16,16],"faces":{' + $colFaces + '}}]}')

# Rock and soil: every state draws nothing.
$hidden = @(
    "stone", "deepslate", "tuff", "granite", "diorite", "andesite", "calcite", "dripstone_block", "smooth_basalt",
    "dirt", "coarse_dirt", "rooted_dirt", "grass_block", "podzol", "mycelium", "gravel", "sand", "red_sand",
    "sandstone", "red_sandstone", "clay", "mud", "terracotta",
    "netherrack", "basalt", "blackstone", "soul_sand", "soul_soil", "end_stone"
)
foreach ($b in $hidden) {
    Write-Json "assets/minecraft/blockstates/$b.json" '{"variants":{"":{"model":"zyntra_xray:block/hidden"}}}'
}

# Ores: one model each, all faces always drawn.
$ores = @(
    "coal_ore", "deepslate_coal_ore", "iron_ore", "deepslate_iron_ore", "copper_ore", "deepslate_copper_ore",
    "gold_ore", "deepslate_gold_ore", "redstone_ore", "deepslate_redstone_ore", "emerald_ore", "deepslate_emerald_ore",
    "lapis_ore", "deepslate_lapis_ore", "diamond_ore", "deepslate_diamond_ore", "nether_gold_ore", "nether_quartz_ore"
)
foreach ($o in $ores) {
    Write-Json "assets/zyntra_xray/models/block/$o.json" ('{"parent":"zyntra_xray:block/full_nocull","textures":{"all":"minecraft:block/' + $o + '"}}')
    Write-Json "assets/minecraft/blockstates/$o.json" ('{"variants":{"":{"model":"zyntra_xray:block/' + $o + '"}}}')
}
# Ancient debris has a top and a side.
Write-Json "assets/zyntra_xray/models/block/ancient_debris.json" '{"parent":"zyntra_xray:block/column_nocull","textures":{"side":"minecraft:block/ancient_debris_side","end":"minecraft:block/ancient_debris_top"}}'
Write-Json "assets/minecraft/blockstates/ancient_debris.json" '{"variants":{"":{"model":"zyntra_xray:block/ancient_debris"}}}'
"Zyntra Xray: $($hidden.Count) blocks hidden, $($ores.Count + 1) ores visible"
