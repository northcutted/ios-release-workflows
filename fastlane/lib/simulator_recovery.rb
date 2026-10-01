# Shared screenshot execution policy for the locked Fastlane adapter.
require "json"
require "open3"
require "fileutils"
require "snapshot/simulator_launchers/simulator_launcher"

module IOSSimulatorRecovery
  class ExecutionFailure < StandardError
    attr_reader :status
    def initialize(status)
      @status = status
      super("Screenshot xcodebuild exited #{status}")
    end
  end

  def self.tool(*arguments)
    stdout, stderr, status = Open3.capture3(ENV.fetch("IOS_RELEASE_PYTHON"), ENV.fetch("IOS_RELEASE_SIMULATOR_TOOL"), *arguments)
    raise "Simulator operation failed: #{stderr}" unless status.success?
    stdout
  end

  module Launcher
    def prepare_simulators_for_launch(device_types, language: nil, locale: nil)
      super
      device_types.each do |name|
        id = Snapshot::TestCommandGenerator.device_udid(name)
        @ios_readiness_sequence = (@ios_readiness_sequence || 0) + 1
        output = File.join(recovery_directory, "readiness-#{id}-#{@ios_readiness_sequence}.json")
        FastlaneCore::UI.message("Waiting for selected simulator #{name} after Fastlane preparation")
        IOSSimulatorRecovery.tool("prepare", id, "--output", output)
      end
    end

    def recovery_directory
      File.join(Snapshot.config[:buildlog_path], "simulator-recovery")
    end

    # Replace Snapshot's generic retries (including its non-65 fallback loop)
    # with one evidence-based recovery per capture job, never assertion retries.
    def execute(_retries = 0, command: nil, language: nil, locale: nil, launch_args: nil, devices: nil)
      original_test_without_building = Snapshot.config[:test_without_building]
      original_clean = Snapshot.config[:clean]
      original_launcher_clean = launcher_config.clean
      recovery_record = nil
      2.times do |attempt|
        begin
          result = FastlaneCore::CommandExecutor.execute(
            command: command, print_all: true, print_command: true,
            prefix: [{prefix: "Running Tests: ", block: proc { |line| line.include?("Touching") }}],
            loading: "Loading...", error: proc { |_output, status| raise ExecutionFailure.new(status) })
          if recovery_record
            record = JSON.parse(File.read(recovery_record)).merge("status" => "recovered")
            File.write(recovery_record, JSON.pretty_generate(record) + "\n")
          end
          return result
        rescue ExecutionFailure => failure
          bundle = File.join(Snapshot.config[:output_directory], "test_output", locale || language, "#{Snapshot.config[:scheme]}.xcresult")
          evidence = File.join(recovery_directory, "failure-#{Time.now.utc.strftime('%Y%m%dT%H%M%S')}-#{attempt + 1}")
          assessment = JSON.parse(IOSSimulatorRecovery.tool("inspect", bundle, "--output", File.join(evidence, "assessment.json")))
          hosted = ENV["GITHUB_ACTIONS"] == "true" && ENV["RUNNER_ENVIRONMENT"] == "github-hosted"
          products = File.join(Snapshot::TestCommandGenerator.derived_data_path, "Build", "Products")
          reusable = Dir.glob(File.join(products, "*.xctestrun")).any?
          allowed = failure.status == 65 && hosted && devices.length == 1 && !@ios_recovery_used && attempt.zero? && assessment["recoverable"] == true && reusable
          unless allowed
            FastlaneCore::UI.crash!("Screenshot execution failed; automatic recovery refused. See #{evidence}")
          end

          @ios_recovery_used = true
          # Preserve the first failure before Snapshot's command generator
          # clears the result path. Keep the compiled products in DerivedData.
          FileUtils.mv(bundle, File.join(evidence, "first-attempt.xcresult"))
          log = xcodebuild_log_path(language: language, locale: locale)
          FileUtils.mv(log, File.join(evidence, "xcodebuild.log")) if File.exist?(log)
          recovery_record = File.join(evidence, "recovery.json")
          File.write(recovery_record, JSON.pretty_generate({status: "retrying", devices: devices, source_sha: ENV["GITHUB_SHA"], reuse_build: true}) + "\n")
          FastlaneCore::UI.important("Recovering recognized XCTest bootstrap failure once; retaining evidence and reusing compiled tests")
          id = Snapshot::TestCommandGenerator.device_udid(devices.first)
          IOSSimulatorRecovery.tool("reset", id)
          Snapshot.config[:clean] = false
          launcher_config.clean = false
          prepare_for_launch(devices, language, locale, launch_args)
          add_media(devices, :photo, launcher_config.add_photos) if launcher_config.add_photos
          add_media(devices, :video, launcher_config.add_videos) if launcher_config.add_videos
          Snapshot.config[:test_without_building] = true
          command = Snapshot::TestCommandGenerator.generate(devices: devices, language: language, locale: locale, log_path: xcodebuild_log_path(language: language, locale: locale))
        end
      end
    ensure
      if recovery_record
        record = JSON.parse(File.read(recovery_record))
        if record["status"] == "retrying"
          File.write(recovery_record, JSON.pretty_generate(record.merge("status" => "failed")) + "\n")
        end
      end
      Snapshot.config[:test_without_building] = original_test_without_building
      Snapshot.config[:clean] = original_clean
      launcher_config.clean = original_launcher_clean
    end
  end

  def self.install!
    raise "Simulator adapter requires reviewed Fastlane 2.240.1" unless Gem.loaded_specs.fetch("fastlane").version.to_s == "2.240.1"
    Snapshot::SimulatorLauncher.prepend(Launcher) unless Snapshot::SimulatorLauncher.ancestors.include?(Launcher)
  end
end

IOSSimulatorRecovery.install!
