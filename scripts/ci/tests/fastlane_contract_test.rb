require 'minitest/autorun'
require 'minitest/mock'
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
    upload_options=nil
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
      upload_options=options
      calls << :metadata
    end
    ReleaseOperations.stub(:new,fake){ff.runner.execute(:stage,:ios)}
    assert_equal [:metadata,:select,:compliance,:precheck,:snapshot],calls
    assert_equal 120,upload_options.fetch(:screenshot_processing_timeout)
  end
  def test_locked_screenshot_recovery_keeps_complete_images_and_retries_pending_only
    deleted=[]
    complete=Object.new
    complete.define_singleton_method(:complete?){true}
    complete.define_singleton_method(:source_file_checksum){'verified-checksum'}
    complete.define_singleton_method(:delete!){raise 'Completed screenshot must be preserved'}
    pending=Object.new
    pending.define_singleton_method(:complete?){false}
    pending.define_singleton_method(:delete!){deleted << :pending}
    iterator=Object.new
    iterator.define_singleton_method(:each_app_screenshot) do |&block|
      [[nil,nil,complete],[nil,nil,pending]].each(&block)
    end
    uploader=Deliver::UploadScreenshots.new
    retried=[]
    uploader.define_singleton_method(:upload_screenshots) do |locales,screens,timeout,tries:|
      retried << [locales,screens,timeout,tries]
    end
    uploader.retry_upload_screenshots_if_needed(iterator,{'COMPLETE'=>159,'UPLOAD_COMPLETE'=>1},160,4,120,[:locale],{})
    assert_equal [:pending],deleted
    assert_equal [[[:locale],{},120,4]],retried
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
