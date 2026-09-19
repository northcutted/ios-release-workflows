require 'minitest/autorun'
require 'tmpdir'
require 'fastlane'
require 'deliver'
require 'precheck'
ENV['IOS_RELEASE_CONFIG'] ||= File.expand_path('../../../examples/picstrip.json', __dir__)
ENV['IOS_RELEASE_CONFIG'] = File.expand_path(ENV['IOS_RELEASE_CONFIG'])
ENV['IOS_APP_ROOT'] = Dir.mktmpdir('release-lane-contract-')
Minitest.after_run { FileUtils.remove_entry(ENV['IOS_APP_ROOT']) }
ENV['STAGED_SNAPSHOT'] = File.join(ENV['IOS_APP_ROOT'], 'staged.json')
Fastlane.load_actions

class FastlaneContractTest < Minitest::Test
  def test_locked_deliver_skips_release_policy_when_metadata_is_skipped
    uploader = Deliver::UploadMetadata.new(skip_metadata:true, automatic_release:true, phased_release:true)
    # Real locked Fastlane returns before accessing any Apple client. Our submission
    # controller must therefore apply policy explicitly, never rely on these flags.
    assert_nil uploader.upload
  end
  def test_actual_stage_lane_selects_build_before_snapshot
    manifest={'version'=>'1.7.0','app_store_build_id'=>'verified','build_number'=>'100.1'}
    calls=[]
    fake=Object.new
    fake.define_singleton_method(:app_id){'app'}
    fake.define_singleton_method(:find_build){{'id'=>'verified','attributes'=>{'processingState'=>'VALID'}}}
    fake.define_singleton_method(:list){|*_args|[]}
    fake.define_singleton_method(:select_build!){calls << :select}
    fake.define_singleton_method(:sync_compliance!){calls << :compliance}
    fake.define_singleton_method(:snapshot){calls << :snapshot; {'app_store_build_id'=>'verified'}}
    ff=Fastlane::FastFile.new(File.expand_path('../../../fastlane/Fastfile',__dir__))
    ff.define_singleton_method(:release_manifest){manifest}
    ff.define_singleton_method(:api_credentials){{}}
    ff.define_singleton_method(:app_store_connect_api_key){|**_|{}}
    ff.define_singleton_method(:strict_precheck){|_|calls << :precheck}
    ff.define_singleton_method(:upload_to_app_store) do |**options|
      raise 'Staging must never submit' if options[:submit_for_review]
      calls << :metadata
    end
    ReleaseOperations.stub(:new,fake){ff.runner.execute(:stage,:ios)}
    assert_equal [:metadata,:select,:compliance,:precheck,:snapshot],calls
  end
  def test_precheck_errors_do_not_turn_into_success
    ff=Fastlane::FastFile.new(File.expand_path('../../../fastlane/Fastfile',__dir__))
    runner=Object.new
    runner.define_singleton_method(:run){raise 'readiness service unavailable'}
    Precheck::Runner.stub(:new,runner) do
      assert_raises(RuntimeError){ff.send(:strict_precheck,{key_id:'example',issuer_id:'example',key:'example'})}
    end
  end
end
