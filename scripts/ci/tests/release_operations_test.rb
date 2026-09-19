require 'minitest/autorun'
require 'tmpdir'
require_relative '../../../fastlane/lib/release_operations'
require_relative '../../../fastlane/lib/build_upload'

class ReleaseOperationsTest < Minitest::Test
  def setup
    @directory = Dir.mktmpdir
    @config = JSON.parse(File.read(File.expand_path('../../../examples/picstrip.json', __dir__)))
    @manifest = {'schema_version'=>3, 'candidate_id'=>'repo:1:1:hash', 'source_sha'=>'a'*40, 'version'=>'1.7.0', 'build_number'=>'100.1', 'ipa_sha256'=>'c'*64,
      'app_store_build_id'=>'verified', 'app'=>{'bundle_id'=>@config['app_store']['bundle_id'], 'team_id'=>@config['team_id']}}
    @encryption=false; @ignore_encryption_write=false
    @selected=nil; @phased=nil; @release_type='MANUAL'; @state='PREPARE_FOR_SUBMISSION'; @submission=nil; @items=[]; @writes=[]
    @guard=ReleaseOperations.new(@manifest, config:@config, client:method(:read), writer:method(:write), receipt_path:File.join(@directory,'receipt.json'))
  end
  def teardown; FileUtils.remove_entry(@directory); end
  def build
    {'id'=>'verified','type'=>'builds','attributes'=>{'version'=>'100.1','processingState'=>'VALID','usesNonExemptEncryption'=>@encryption}, 'relationships'=>{'app'=>{'data'=>{'id'=>'app'}},'preReleaseVersion'=>{'data'=>{'id'=>'prerelease'}}}}
  end
  def read(path, query={})
    case path
    when '/v1/apps' then {'data'=>[{'id'=>'app'}]}
    when '/v1/builds' then {'data'=>[build], 'included'=>[{'id'=>'prerelease','type'=>'preReleaseVersions','attributes'=>{'version'=>'1.7.0','platform'=>'IOS'}}]}
    when '/v1/apps/app/appStoreVersions' then {'data'=>[{'id'=>'version','attributes'=>{'versionString'=>'1.7.0','releaseType'=>@release_type,'appVersionState'=>@state}}]}
    when '/v1/appStoreVersions/version/build' then {'data'=>@selected}
    when '/v1/appStoreVersions/version/appStoreVersionPhasedRelease' then {'data'=>@phased}
    when '/v1/apps/app/reviewSubmissions' then {'data'=>[@submission].compact}
    when '/v1/reviewSubmissions/submission' then {'data'=>@submission}
    when '/v1/reviewSubmissions/submission/items' then {'data'=>@items}
    when '/v1/appStoreVersions/version/appStoreVersionLocalizations','/v1/apps/app/appInfos' then {'data'=>[]}
    else raise "Unexpected GET #{path}"
    end
  end
  def write(method, path, payload)
    @writes << [method,path,payload]
    case path
    when '/v1/builds/verified' then @encryption=payload['data']['attributes']['usesNonExemptEncryption'] unless @ignore_encryption_write; {}
    when '/v1/appStoreVersions/version/relationships/build' then @selected=build; {}
    when '/v1/appStoreVersions/version' then @release_type=payload['data']['attributes']['releaseType']; {}
    when '/v1/appStoreVersionPhasedReleases' then @phased={'id'=>'phased','attributes'=>{'phasedReleaseState'=>'INACTIVE'}}; {'data'=>@phased}
    when '/v1/reviewSubmissions' then @submission={'id'=>'submission','attributes'=>{'state'=>'READY_FOR_REVIEW'}}; {'data'=>@submission}
    when '/v1/reviewSubmissionItems'
      item={'id'=>'item','relationships'=>{'appStoreVersion'=>{'data'=>{'id'=>'version'}}}}; @items << item
      if @fail_after_item; @fail_after_item=false; raise 'connection lost after Apple accepted item'; end
      {'data'=>item}
    when '/v1/reviewSubmissions/submission' then @state='WAITING_FOR_REVIEW'; @submission['attributes']['state']='WAITING_FOR_REVIEW'; {'data'=>@submission}
    else raise "Unexpected #{method} #{path}"
    end
  end
  def production
    before=ENV['RELEASE_ENVIRONMENT']; ENV['RELEASE_ENVIRONMENT']='production'; yield
  ensure
    before ? ENV['RELEASE_ENVIRONMENT']=before : ENV.delete('RELEASE_ENVIRONMENT')
  end
  def test_matching_encryption_declaration_is_read_only_for_both_declared_values
    @config['app_store']['age_rating']={}
    [false,true].each do |value|
      @encryption=value; @config['targets'].first['non_exempt_encryption']=value
      @guard.sync_compliance!
      assert_empty @writes
    end
  end
  def test_missing_encryption_declaration_is_written_once_and_read_back
    @config['app_store']['age_rating']={}; @encryption=nil
    @guard.sync_compliance!
    @guard.sync_compliance!
    assert_equal false,@encryption
    assert_equal 1,@writes.length
  end
  def test_encryption_update_requires_matching_readback
    @config['app_store']['age_rating']={}; @encryption=nil; @ignore_encryption_write=true
    assert_raises(RuntimeError){@guard.sync_compliance!}
  end
  def test_fresh_draft_explicitly_attaches_build_and_rechecks
    @guard.select_build!
    assert_equal 'verified', @selected['id']
    assert_equal '/v1/appStoreVersions/version/relationships/build', @writes.first[1]
    assert_equal 'verified', @guard.snapshot['app_store_build_id']
    count=@writes.length; @guard.select_build!; assert_equal count,@writes.length
  end
  def test_refuses_to_replace_selected_build
    @selected=build.merge('id'=>'foreign')
    assert_raises(RuntimeError){@guard.select_build!}; assert_empty @writes
  end
  def test_automatic_and_phased_policy_is_applied_with_readback
    @selected=build; production{@guard.submit!(timeout:0,interval:0)}
    assert_equal 'AFTER_APPROVAL', @release_type
    refute_nil @phased
    assert_equal 'submitted', JSON.parse(File.read(File.join(@directory,'receipt.json')))['status']
  end
  def test_production_gate_is_required
    @selected=build
    old=ENV.delete('RELEASE_ENVIRONMENT')
    assert_raises(RuntimeError){@guard.submit!(timeout:0,interval:0)}
    assert_empty @writes
  ensure
    ENV['RELEASE_ENVIRONMENT']=old if old
  end
  def test_resume_after_item_creation_does_not_duplicate_or_replace_it
    @selected=build; @fail_after_item=true
    production do
      assert_raises(RuntimeError){@guard.submit!(timeout:0,interval:0)}
      @guard.submit!(timeout:0,interval:0)
    end
    assert_equal 1,@items.length
    assert_equal 1,@writes.count{|w|w[1]=='/v1/reviewSubmissionItems'}
  end
  def test_unrelated_review_item_is_never_removed
    @selected=build; @submission={'id'=>'submission','attributes'=>{'state'=>'READY_FOR_REVIEW'}}
    @items=[{'id'=>'foreign','relationships'=>{'appStoreVersion'=>{'data'=>{'id'=>'other-version'}}}}]
    production{assert_raises(RuntimeError){@guard.submit!(timeout:0,interval:0)}}
    assert_equal 'PREPARE_FOR_SUBMISSION',@state
    assert_equal 'foreign',@items.first['id']
    assert_empty @writes
  end
  def test_not_ready_submission_is_not_sent_and_can_resume
    @selected=build
    @submission={'id'=>'submission','attributes'=>{'state'=>'UNRESOLVED_ISSUES'}}
    @items=[{'id'=>'item','relationships'=>{'appStoreVersion'=>{'data'=>{'id'=>'version'}}}}]
    production do
      assert_raises(RuntimeError){@guard.submit!(timeout:0,interval:0)}
      refute @writes.any?{|write|write[1]=='/v1/reviewSubmissions/submission'}
      @submission['attributes']['state']='READY_FOR_REVIEW'
      @guard.submit!(timeout:0,interval:0)
    end
    assert_equal 1,@items.length
  end
  def test_submitted_retry_is_read_only
    @selected=build; @state='WAITING_FOR_REVIEW'
    production{@guard.submit!(timeout:0,interval:0)}
    assert_empty @writes
  end
  def test_receipt_cannot_be_reused_for_another_candidate
    @guard.checkpoint('status'=>'ready')
    assert_raises(RuntimeError){ReleaseOperations.new(@manifest.merge('ipa_sha256'=>'f'*64),config:@config,receipt_path:File.join(@directory,'receipt.json'))}
  end
  def test_testflight_group_assignment_is_checked_and_idempotent
    @config['app_store']['testflight_groups']=['group']
    assigned=[];writes=[]
    reader=lambda do |path,query={}|
      case path
      when '/v1/betaGroups/group' then {'data'=>{'relationships'=>{'app'=>{'data'=>{'id'=>'app'}}},'attributes'=>{'isInternalGroup'=>true}}}
      when '/v1/betaGroups/group/builds' then {'data'=>assigned}
      else read(path,query)
      end
    end
    writer=lambda do |method,path,payload|
      writes<<[method,path];assigned.replace(payload['data']);{}
    end
    guard=ReleaseOperations.new(@manifest,config:@config,client:reader,writer:writer,receipt_path:File.join(@directory,'groups.json'))
    2.times{guard.testflight_groups!}
    assert_equal 1,writes.length
    assert_equal 'verified',assigned.first['id']
  end
  def test_foreign_testflight_group_stops_before_write
    @config['app_store']['testflight_groups']=['group']
    reader=lambda do |path,query={}|
      path=='/v1/betaGroups/group' ? {'data'=>{'relationships'=>{'app'=>{'data'=>{'id'=>'foreign'}}}}} : read(path,query)
    end
    guard=ReleaseOperations.new(@manifest,config:@config,client:reader,writer:method(:write),receipt_path:File.join(@directory,'groups.json'))
    assert_raises(RuntimeError){guard.testflight_groups!}
    assert_empty @writes
  end
  def test_range_io_does_not_send_bytes_from_next_upload_part
    require 'stringio'
    input=StringIO.new('abcdefghij'); input.seek(2)
    stream=BuildUpload::RangeIO.new(input,3)
    assert_equal 'cde',stream.read(10)
    assert_nil stream.read(1)
  end
end

class BuildUploadRecoveryTest < Minitest::Test
  def setup
    @directory=Dir.mktmpdir
    @ipa=File.join(@directory,'application.ipa');File.binwrite(@ipa,'verified ipa bytes')
    @config=JSON.parse(File.read(File.expand_path('../../../examples/picstrip.json',__dir__)))
    @config['upload_adapter']='build-uploads'
    @manifest={'schema_version'=>3,'candidate_id'=>'candidate','source_sha'=>'a'*40,'version'=>'1.7.0','build_number'=>'100.1','ipa_sha256'=>Digest::SHA256.file(@ipa).hexdigest,'app'=>{'bundle_id'=>@config['app_store']['bundle_id'],'team_id'=>@config['team_id']}}
    @upload=nil;@file=nil;@puts=0;@posts=[]
    @uploader=controller
  end
  def teardown;FileUtils.remove_entry(@directory);end
  def controller
    object=BuildUpload.new(@manifest,config:@config,client:method(:read),writer:method(:write),receipt_path:File.join(@directory,'receipt.json'))
    test=self
    object.define_singleton_method(:transfer_part){|ipa,operation|test.transfer(ipa,operation)}
    object
  end
  def transfer(_ipa,_operation)
    @puts+=1
    if @fail_part;@fail_part=false;raise IOError,'lost transfer response';end
  end
  def read(path,query={})
    case path
    when '/v1/apps' then {'data'=>[{'id'=>'app'}]}
    when '/v1/apps/app/buildUploads' then {'data'=>[@upload].compact}
    when '/v1/builds' then {'data'=>[@build].compact, 'included'=>[{'id'=>'prerelease','type'=>'preReleaseVersions','attributes'=>{'version'=>'1.7.0','platform'=>'IOS'}}]}
    when '/v1/buildUploads/upload'
      response=Marshal.load(Marshal.dump(@upload))
      # Apple omits relationship linkage unless explicitly included.
      response['relationships'].delete('build') unless query.fetch('include','').split(',').include?('build')
      {'data'=>response,'included'=>[@file].compact}
    else raise "Unexpected GET #{path}"
    end
  end
  def write(_method,path,payload)
    case path
    when '/v1/buildUploads'
      @posts<<path
      @upload={'id'=>'upload','attributes'=>payload['data']['attributes'].merge('state'=>{'state'=>'AWAITING_UPLOAD','errors'=>[],'warnings'=>[],'infos'=>[]}),'relationships'=>{}}
      {'data'=>@upload}
    when '/v1/buildUploadFiles'
      @posts<<path
      @file={'id'=>'file','type'=>'buildUploadFiles','attributes'=>payload['data']['attributes'].merge('assetDeliveryState'=>{'state'=>'AWAITING_UPLOAD'},'uploadOperations'=>[{'method'=>'PUT','offset'=>0,'length'=>File.size(@ipa)}])}
      @upload['relationships']['assetFile']={'data'=>{'id'=>'file'}}
      {'data'=>@file}
    when '/v1/buildUploadFiles/file'
      @file['attributes'].merge!(payload['data']['attributes'])
      @file['attributes']['assetDeliveryState']={'state'=>'UPLOAD_COMPLETE'}
      if @fail_commit;@fail_commit=false;raise IOError,'lost commit response';end
      {'data'=>@file}
    else raise "Unexpected mutation #{path}"
    end
  end
  def test_partial_transfer_reuses_reservation_and_file
    @fail_part=true
    assert_raises(IOError){@uploader.upload!(@ipa)}
    controller.upload!(@ipa)
    assert_equal 2,@posts.length
    assert_equal 2,@puts
    assert_equal @manifest['ipa_sha256'],@file.dig('attributes','sourceFileChecksums','file','hash')
  end
  def test_lost_commit_response_does_not_duplicate_or_retransfer
    @fail_commit=true
    assert_raises(IOError){@uploader.upload!(@ipa)}
    controller.upload!(@ipa)
    assert_equal 2,@posts.length
    assert_equal 1,@puts
  end
  def test_conflicting_committed_file_is_rejected
    @uploader.upload!(@ipa)
    @file['attributes']['sourceFileChecksums']['file']['hash']='f'*64
    assert_raises(RuntimeError){controller.upload!(@ipa)}
    assert_equal 1,@puts
  end
  def complete_upload
    @uploader.upload!(@ipa)
    @upload['attributes']['state']['state']='COMPLETE'
    @upload['relationships']['build']={'data'=>{'id'=>'verified'}}
    @build={'id'=>'verified','attributes'=>{'version'=>'100.1','processingState'=>'VALID'},'relationships'=>{'app'=>{'data'=>{'id'=>'app'}},'preReleaseVersion'=>{'data'=>{'id'=>'prerelease'}}}}
  end
  def test_nested_apple_completion_returns_exact_build_and_upload_ids
    complete_upload
    status=controller.wait_for_verified_processing!(timeout:0,interval:0)
    assert_equal 'VALID',status['processing_state']
    assert_equal 'verified',status['app_store_build_id']
    assert_equal 'upload',status['upload_id']
    assert_equal 'file',status['upload_file_id']
  end
  def test_nested_apple_failure_stops_immediately
    complete_upload
    @upload['attributes']['state']['state']='FAILED'
    assert_match(/Apple rejected upload/,assert_raises(RuntimeError){controller.wait_for_verified_processing!(timeout:0,interval:0)}.message)
  end
  def test_completed_upload_cannot_substitute_a_build
    complete_upload
    @build['id']='foreign'
    assert_match(/conflicts with upload/,assert_raises(RuntimeError){controller.wait_for_verified_processing!(timeout:0,interval:0)}.message)
  end
  def test_transporter_receipt_requires_upload_file_identity
    complete_upload
    @config['upload_adapter']='transporter'
    controller.checkpoint('adapter'=>'transporter','status'=>'transferred')
    @file=nil
    assert_match(/Missing Apple upload file identity/,assert_raises(RuntimeError){controller.reconcile_accepted!}.message)
  end
  def test_tampered_local_ipa_stops_before_any_apple_write
    File.binwrite(@ipa,'substituted')
    assert_raises(RuntimeError){@uploader.upload!(@ipa)}
    assert_empty @posts
  end
end
