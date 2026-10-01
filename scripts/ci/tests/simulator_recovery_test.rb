require "minitest/autorun"
require "minitest/mock"
require "tmpdir"
require "fastlane"
FastlaneCore::Helper.stub(:test?, true) { require "snapshot" }
require_relative "../../../fastlane/lib/simulator_recovery"

class SimulatorPreparation
  attr_reader :events
  def initialize
    @events = []
  end
  def prepare_simulators_for_launch(_devices, language: nil, locale: nil)
    @events << :fastlane_prepared
  end
  def prepare_for_launch(devices, language, locale, _args)
    prepare_simulators_for_launch(devices, language: language, locale: locale)
  end
  def xcodebuild_log_path(language:, locale:)
    File.join(Snapshot.config[:buildlog_path], "capture.log")
  end
end

class SimulatorHarness < SimulatorPreparation
  prepend IOSSimulatorRecovery::Launcher
end

class SimulatorRecoveryTest < Minitest::Test
  ID = "C71FB2C5-952C-4E6D-A2AD-1ADAE3B28FA1"

  def with_adapter(statuses:, recoverable: true, hosted: true, compiled: true)
    Dir.mktmpdir do |tmp|
      config = {buildlog_path: File.join(tmp, "logs"), output_directory: File.join(tmp, "screenshots"), scheme: "AppScreenshots", test_without_building: nil}
      FileUtils.mkdir_p(config[:buildlog_path])
      bundle = File.join(config[:output_directory], "test_output/en-US/AppScreenshots.xcresult")
      FileUtils.mkdir_p(bundle)
      File.write(File.join(bundle, "original"), "first failure")
      File.write(File.join(config[:buildlog_path], "capture.log"), "first log")
      products = File.join(tmp, "Build/Products")
      FileUtils.mkdir_p(products)
      File.write(File.join(products, "App.xctestrun"), "compiled tests") if compiled
      adapter = SimulatorHarness.new
      calls, operations = [], []
      executor = proc do |**options|
        calls << options[:command]
        status = statuses.shift
        raise "Unexpected extra execution" if status.nil?
        options[:error].call("output", status) unless status.zero?
        "success"
      end
      tool = proc do |operation, *arguments|
        operations << operation
        if operation == "inspect"
          FileUtils.mkdir_p(File.dirname(arguments.last))
          JSON.generate(recoverable: recoverable)
        else
          adapter.events << :ready if operation == "prepare"
          "{}"
        end
      end
      previous = ENV.to_h
      ENV.update("GITHUB_ACTIONS" => hosted ? "true" : "false", "RUNNER_ENVIRONMENT" => "github-hosted")
      Snapshot.stub(:config, config) do
        Snapshot::TestCommandGenerator.stub(:derived_data_path, tmp) do
          Snapshot::TestCommandGenerator.stub(:device_udid, ID) do
            Snapshot::TestCommandGenerator.stub(:generate, ["test-without-building"]) do
              IOSSimulatorRecovery.stub(:tool, tool) do
                FastlaneCore::CommandExecutor.stub(:execute, executor) do
                  FastlaneCore::UI.ui_object.stub(:crash!, proc { |message| raise RuntimeError, message }) do
                    yield adapter, calls, operations, config, tmp
                  end
                end
              end
            end
          end
        end
      end
    ensure
      ENV.replace(previous) if previous
    end
  end

  def capture(adapter)
    adapter.execute(command: ["build", "test"], language: "en-US", locale: nil, launch_args: [""], devices: ["device"])
  end

  def test_readiness_happens_after_fastlane_preparation
    with_adapter(statuses: []) do |adapter, _, operations|
      adapter.prepare_simulators_for_launch(["device"], language: "en-US")
      assert_equal [:fastlane_prepared, :ready], adapter.events
      assert_equal ["prepare"], operations
    end
  end

  def test_one_bootstrap_recovery_preserves_evidence_and_reuses_compiled_tests
    with_adapter(statuses: [65, 0]) do |adapter, calls, operations, config, tmp|
      assert_equal "success", capture(adapter)
      assert_equal [["build", "test"], ["test-without-building"]], calls
      assert_equal ["inspect", "reset", "prepare"], operations
      assert_equal [:fastlane_prepared, :ready], adapter.events
      assert_nil config[:test_without_building]
      record = Dir.glob(File.join(tmp, "logs/simulator-recovery/**/recovery.json")).first
      assert_equal "recovered", JSON.parse(File.read(record))["status"]
      assert_equal "first failure", File.read(File.join(File.dirname(record), "first-attempt.xcresult/original"))
      assert_equal "first log", File.read(File.join(File.dirname(record), "xcodebuild.log"))
      assert_equal "compiled tests", File.read(File.join(tmp, "Build/Products/App.xctestrun"))
    end
  end

  def test_second_startup_failure_is_final_and_retained
    with_adapter(statuses: [65, 65]) do |adapter, calls, operations, config, tmp|
      assert_raises(RuntimeError) { capture(adapter) }
      assert_equal 2, calls.length
      assert_equal 1, operations.count("reset")
      assert_nil config[:test_without_building]
      record = Dir.glob(File.join(tmp, "logs/simulator-recovery/**/recovery.json")).first
      assert_equal "failed", JSON.parse(File.read(record))["status"]
    end
  end

  def test_assertions_local_runs_missing_builds_and_non_test_errors_do_not_retry
    [{recoverable: false}, {hosted: false}, {compiled: false}, {status: 70}].each do |options|
      status = options.delete(:status) || 65
      with_adapter(statuses: [status], **options) do |adapter, calls, operations|
        assert_raises(RuntimeError) { capture(adapter) }
        assert_equal 1, calls.length
        refute_includes operations, "reset"
      end
    end
  end

  def test_only_one_recovery_is_available_across_the_capture_job
    with_adapter(statuses: [65, 0, 65]) do |adapter, calls, operations|
      capture(adapter)
      assert_raises(RuntimeError) { capture(adapter) }
      assert_equal 3, calls.length
      assert_equal 1, operations.count("reset")
    end
  end
end
