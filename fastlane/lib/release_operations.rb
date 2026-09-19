require_relative "release_guard"

# Apple writes live here; the caller must first authenticate the manifest and enter
# the corresponding protected environment. GET-only validation stays in ReleaseGuard.
class ReleaseOperations < ReleaseGuard
  def initialize(manifest, config: nil, client: nil, writer: nil, receipt_path: nil)
    super(manifest, config: config, client: client)
    @writer = writer
    @receipt_path = receipt_path || ENV.fetch("OPERATION_RECEIPT", "build/operation.json")
    @receipt = File.exist?(@receipt_path) ? JSON.parse(File.read(@receipt_path)) : {}
    identity = manifest.slice("candidate_id", "source_sha", "version", "build_number", "ipa_sha256", "app_store_build_id")
    raise "Operation receipt belongs to another candidate" if @receipt["identity"] && @receipt["identity"] != identity
    @receipt["identity"] = identity
  end

  def checkpoint(values)
    @receipt.merge!(values)
    ReleaseGuard.save(@receipt_path, @receipt)
  end

  def mutate(method, path, data)
    return @writer.call(method, path, data) if @writer
    uri = URI("#{ROOT}#{path}")
    raise "Unexpected mutation URL" unless path.start_with?("/v1/") && uri.host == "api.appstoreconnect.apple.com"
    key = OpenSSL::PKey.read(ENV.fetch("APP_STORE_CONNECT_API_KEY_CONTENT").gsub('\\n', "\n"))
    token = JWT.encode({iss: ENV.fetch("APP_STORE_CONNECT_API_KEY_ISSUER_ID"), iat: Time.now.to_i - 10,
                       exp: Time.now.to_i + 600, aud: "appstoreconnect-v1"}, key, "ES256",
                      {kid: ENV.fetch("APP_STORE_CONNECT_API_KEY_ID"), typ: "JWT"})
    request = {"POST" => Net::HTTP::Post, "PATCH" => Net::HTTP::Patch}.fetch(method).new(uri)
    request["Authorization"] = "Bearer #{token}"
    request["Content-Type"] = "application/json"
    request.body = JSON.generate(data)
    # Never blindly retry POST: after ambiguous responses reconcile Apple state.
    response = Net::HTTP.start(uri.host, uri.port, use_ssl: true, open_timeout: 15, read_timeout: 90) { |http| http.request(request) }
    raise "App Store #{method} #{path}: HTTP #{response.code}; reconcile before retry" unless response.is_a?(Net::HTTPSuccess)
    response.body.to_s.empty? ? {} : JSON.parse(response.body)
  end

  def select_build!
    build = find_build
    raise "Processed build identity changed" unless build && build["id"] == @manifest.fetch("app_store_build_id") && build.dig("attributes", "processingState") == "VALID"
    v = version
    selected = get("/v1/appStoreVersions/#{v.fetch('id')}/build")["data"]
    raise "Refusing to replace a different selected build" if selected && selected["id"] != build["id"]
    unless selected
      mutate("PATCH", "/v1/appStoreVersions/#{v.fetch('id')}/relationships/build", {"data" => {"type" => "builds", "id" => build.fetch("id")}})
    end
    verify_selected_build!
    checkpoint("app_store_version_id" => v.fetch("id"), "stage" => "build-selected")
  end

  def apply_release_policy!
    v = verify_selected_build!
    policy = @config.fetch("app_store")
    mutate("PATCH", "/v1/appStoreVersions/#{v.fetch('id')}", {"data" => {"type" => "appStoreVersions", "id" => v.fetch("id"), "attributes" => {"releaseType" => policy.fetch("release_type")}}})
    phased = optional_get("/v1/appStoreVersions/#{v.fetch('id')}/appStoreVersionPhasedRelease")["data"]
    if policy.fetch("phased_release") && !phased
      mutate("POST", "/v1/appStoreVersionPhasedReleases", {"data" => {"type" => "appStoreVersionPhasedReleases", "attributes" => {"phasedReleaseState" => "INACTIVE"}, "relationships" => {"appStoreVersion" => {"data" => {"type" => "appStoreVersions", "id" => v.fetch("id")}}}}})
    elsif !policy.fetch("phased_release") && phased
      raise "Existing phased release conflicts with configured non-phased policy; resolve explicitly"
    end
    readback = version
    phased = optional_get("/v1/appStoreVersions/#{v.fetch('id')}/appStoreVersionPhasedRelease")["data"]
    raise "Release policy readback mismatch" unless readback.dig("attributes", "releaseType") == policy["release_type"] && !!phased == policy["phased_release"]
    checkpoint("release_policy" => {"release_type" => policy["release_type"], "phased_release_id" => phased && phased["id"]})
  end

  def submit!(timeout: 300, interval: 5)
    raise "Production approval is required" unless ENV["RELEASE_ENVIRONMENT"] == "production"
    v = verify_selected_build!
    state = v.dig("attributes", "appVersionState") || v.dig("attributes", "appStoreState")
    if SUBMITTED_STATES.include?(state)
      checkpoint("status" => "submitted", "readback" => snapshot)
      return
    end
    submissions = list("/v1/apps/#{app_id}/reviewSubmissions", "filter[platform]" => "IOS")
    active = submissions.reject { |s| %w[COMPLETE CANCELED].include?(s.dig("attributes", "state")) }
    raise "Ambiguous or foreign review submission" if active.length > 1
    submission = active.first
    if @receipt["review_submission_id"] && submission && submission["id"] != @receipt["review_submission_id"]
      raise "Review submission identity changed"
    end
    unless submission
      submission = mutate("POST", "/v1/reviewSubmissions", {"data" => {"type" => "reviewSubmissions", "attributes" => {"platform" => "IOS"}, "relationships" => {"app" => {"data" => {"type" => "apps", "id" => app_id}}}}}).fetch("data")
    end
    checkpoint("review_submission_id" => submission.fetch("id"), "status" => "preparing")
    items = list("/v1/reviewSubmissions/#{submission.fetch('id')}/items", "include" => "appStoreVersion")
    raise "Submission contains unrelated review items" unless items.all? { |i| i.dig("relationships", "appStoreVersion", "data", "id") == v["id"] } && items.length <= 1
    apply_release_policy!
    if items.empty?
      item = mutate("POST", "/v1/reviewSubmissionItems", {"data" => {"type" => "reviewSubmissionItems", "relationships" => {"reviewSubmission" => {"data" => {"type" => "reviewSubmissions", "id" => submission.fetch("id")}}, "appStoreVersion" => {"data" => {"type" => "appStoreVersions", "id" => v.fetch("id")}}}}}).fetch("data")
      checkpoint("review_item_id" => item.fetch("id"))
    else
      checkpoint("review_item_id" => items.first.fetch("id"))
    end
    deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + timeout
    loop do
      submission = get("/v1/reviewSubmissions/#{submission.fetch('id')}").fetch("data")
      state = submission.dig("attributes", "state")
      break if %w[READY_FOR_REVIEW WAITING_FOR_REVIEW IN_REVIEW].include?(state)
      raise "Apple review submission is not ready; resume with recorded IDs" if Process.clock_gettime(Process::CLOCK_MONOTONIC) >= deadline
      sleep(interval)
    end
    verify_selected_build!
    mutate("PATCH", "/v1/reviewSubmissions/#{submission.fetch('id')}", {"data" => {"type" => "reviewSubmissions", "id" => submission.fetch("id"), "attributes" => {"submitted" => true}}}) unless %w[WAITING_FOR_REVIEW IN_REVIEW].include?(submission.dig("attributes", "state"))
    deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + timeout
    loop do
      readback = snapshot
      state = readback.fetch("version_attributes")["appVersionState"] || readback.fetch("version_attributes")["appStoreState"]
      if SUBMITTED_STATES.include?(state)
        checkpoint("status" => "submitted", "readback" => readback)
        return
      end
      raise "Apple has not confirmed submission; resume with recorded operation IDs" if Process.clock_gettime(Process::CLOCK_MONOTONIC) >= deadline
      sleep(interval)
    end
  end

  def testflight_groups!
    build_id = @manifest.fetch("app_store_build_id")
    @config.fetch("app_store").fetch("testflight_groups").each do |id|
      raise "Invalid beta group ID" unless id.match?(/\A[A-Za-z0-9-]+\z/)
      group = get("/v1/betaGroups/#{id}", "include" => "app").fetch("data")
      raise "Beta group belongs to another app" unless group.dig("relationships", "app", "data", "id") == app_id
      unless group.dig("attributes", "isInternalGroup")
        review = optional_get("/v1/builds/#{build_id}/betaAppReviewSubmission")["data"]
        unless review
          review = mutate("POST", "/v1/betaAppReviewSubmissions", {"data" => {"type" => "betaAppReviewSubmissions", "relationships" => {"build" => {"data" => {"type" => "builds", "id" => build_id}}}}}).fetch("data")
        end
        checkpoint("beta_review_submission_id" => review.fetch("id"))
        state = review.dig("attributes", "betaReviewState")
        raise "External TestFlight review rejected; owner action required" if state == "REJECTED"
      end
      builds = list("/v1/betaGroups/#{id}/builds", "filter[version]" => @manifest.fetch("build_number"))
      unless builds.any? { |b| b["id"] == build_id }
        mutate("POST", "/v1/betaGroups/#{id}/relationships/builds", {"data" => [{"type" => "builds", "id" => build_id}]})
      end
      raise "TestFlight group assignment readback failed" unless list("/v1/betaGroups/#{id}/builds", "filter[version]" => @manifest.fetch("build_number")).any? { |b| b["id"] == build_id }
    end
  end

  def sync_compliance!
    build = find_build
    raise "Wrong compliance build" unless build && build["id"] == @manifest.fetch("app_store_build_id")
    encryption = @config.fetch("targets").first.fetch("non_exempt_encryption")
    # Apple can reject even an identical PATCH once the archive supplied this
    # declaration. Preserve matching facts and always verify the live readback.
    unless build.dig("attributes", "usesNonExemptEncryption") == encryption
      mutate("PATCH", "/v1/builds/#{build.fetch('id')}", {"data" => {"type" => "builds", "id" => build.fetch("id"), "attributes" => {"usesNonExemptEncryption" => encryption}}})
    end
    raise "Encryption declaration readback mismatch" unless find_build.dig("attributes", "usesNonExemptEncryption") == encryption
    ratings = @config.fetch("app_store").fetch("age_rating")
    return if ratings.empty?
    infos = list("/v1/apps/#{app_id}/appInfos")
    editable = infos.select { |i| %w[PREPARE_FOR_SUBMISSION READY_FOR_REVIEW].include?(i.dig("attributes", "appStoreState")) }
    raise "Expected one editable app information record" unless editable.length == 1
    rating = get("/v1/appInfos/#{editable.first.fetch('id')}/ageRatingDeclaration").fetch("data")
    mutate("PATCH", "/v1/ageRatingDeclarations/#{rating.fetch('id')}", {"data" => {"type" => "ageRatingDeclarations", "id" => rating.fetch("id"), "attributes" => ratings}})
    actual = get("/v1/ageRatingDeclarations/#{rating.fetch('id')}").fetch("data").fetch("attributes")
    raise "Age rating readback mismatch" unless ratings.all? { |k, v| actual[k] == v }
  end
end
