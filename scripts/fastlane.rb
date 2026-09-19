require "fastlane"
root = File.expand_path("..", __dir__)
ENV["FASTLANE_SKIP_DOCS"] = "true"
ENV["FASTLANE_SKIP_UPDATE_CHECK"] = "true"
ENV["FASTLANE_OPT_OUT_USAGE"] = "true"
Dir.chdir(root) do
  Fastlane.load_actions
  Fastlane::LaneManager.cruise_lane("ios", ARGV.fetch(0), {}, File.join(root, "fastlane/Fastfile"))
end
