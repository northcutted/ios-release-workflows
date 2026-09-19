require 'minitest/autorun'
require 'minitest/mock'
require 'tmpdir'
require 'fastlane'
require_relative '../../../fastlane/actions/sync_accessibility_declarations'

class AccessibilityContractTest < Minitest::Test
  Action = Fastlane::Actions::SyncAccessibilityDeclarationsAction
  def setup
    @directory=Dir.mktmpdir
    @desired={'deviceFamily'=>'IPHONE'}.merge(Action::FEATURE_KEYS.to_h { |key| [key,false] })
    @desired['supportsVoiceover']=true
    @path=File.join(@directory,'accessibility.json')
    File.write(@path,JSON.generate({'declarations'=>[@desired]}))
    @attributes=@desired.merge('state'=>'DRAFT'); @exists=true; @writes=[]; @reject_readback=false
  end
  def teardown; FileUtils.remove_entry(@directory); end
  def request(_token,method,path,query:{},body:nil)
    if method==:get
      case path
      when '/v1/apps' then {'data'=>[{'id'=>'app'}]}
      when '/v1/apps/app/accessibilityDeclarations' then {'data'=>@exists ? [{'id'=>'declaration','attributes'=>@attributes.dup}] : []}
      when '/v1/accessibilityDeclarations/declaration'
        attrs=@attributes.dup
        attrs['supportsVoiceover']=false if @reject_readback
        {'data'=>{'id'=>'declaration','attributes'=>attrs}}
      else raise "Unexpected GET #{path}"
      end
    else
      @writes << [method,path,body]
      attrs=body.fetch(:data).fetch(:attributes)
      if method==:patch
        raise 'Apple rejects immutable deviceFamily on PATCH' if attrs.key?('deviceFamily')
        raise 'Unexpected update fields' unless (attrs.keys-Action::FEATURE_KEYS).empty?
      else
        assert_equal :post,method
        assert_equal 'IPHONE',attrs.fetch('deviceFamily')
        @exists=true
      end
      @attributes.merge!(attrs)
      {'data'=>{'id'=>'declaration','attributes'=>@attributes.dup}}
    end
  end
  def sync
    Action.stub(:make_token,'not-a-credential') do
      Action.stub(:request,method(:request)) { Action.run(config_path:@path,app_identifier:'example.app') }
    end
  end
  def test_matching_draft_and_published_declarations_are_read_only
    %w[DRAFT PUBLISHED].each do |state|
      @attributes['state']=state; sync
      assert_empty @writes
    end
  end
  def test_changed_draft_only_patches_mutable_features
    @attributes['supportsVoiceover']=false
    sync
    assert_equal 1,@writes.length
    refute @writes.first.last[:data][:attributes].key?('deviceFamily')
    assert_equal true,@attributes['supportsVoiceover']
  end
  def test_creation_includes_device_family_and_reads_it_back
    @exists=false
    sync
    assert_equal :post,@writes.first.first
  end
  def test_wrong_readback_fails_closed
    @reject_readback=true
    assert_raises(FastlaneCore::Interface::FastlaneError){sync}
  end
  def test_changed_published_facts_are_not_silently_rewritten
    @attributes['state']='PUBLISHED'; @attributes['supportsVoiceover']=false
    assert_raises(FastlaneCore::Interface::FastlaneError){sync}
    assert_empty @writes
  end
end
