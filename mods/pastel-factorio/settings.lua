-- Startup settings: change them in Settings > Mod settings > Startup.
-- Changing a startup setting makes the game restart to apply it.
data:extend({
  {
    type = "bool-setting",
    name = "pastel-factorio-graphics",
    setting_type = "startup",
    default_value = true,
    order = "a"
  },
  {
    type = "double-setting",
    name = "pastel-factorio-water-strength",
    setting_type = "startup",
    default_value = 1.0,
    minimum_value = 0,
    maximum_value = 1,
    order = "b"
  },
  {
    type = "string-setting",
    name = "pastel-factorio-water-color",
    setting_type = "startup",
    default_value = "#7FDDE6",
    order = "c"
  },
  {
    type = "string-setting",
    name = "pastel-factorio-water-deep-color",
    setting_type = "startup",
    default_value = "#4FC3D9",
    order = "d"
  },
})
