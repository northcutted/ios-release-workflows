require_relative "release_operations"

class BuildUpload < ReleaseOperations
  def uploads
    list("/v1/apps/#{app_id}/buildUploads", "filter[cfBundleShortVersionString]" => @manifest.fetch("version"),
         "filter[cfBundleVersion]" => @manifest.fetch("build_number"), "filter[platform]" => "IOS")
  end

  def upload_state(upload)
    value = upload.dig("attributes", "state")
    state = value.is_a?(Hash) && value["state"]
    raise "Missing Apple upload state" unless state.is_a?(String) && !state.empty?
    state
  end

  def exact_upload
    values = uploads
    raise "Ambiguous existing upload" unless values.length <= 1
    upload = values.first
    if upload
      attrs = upload.fetch("attributes")
      raise "Wrong upload version/platform" unless attrs["cfBundleShortVersionString"] == @manifest["version"] && attrs["cfBundleVersion"] == @manifest["build_number"] && attrs["platform"] == "IOS"
      raise "Apple rejected upload" if upload_state(upload) == "FAILED"
      raise "Upload ID changed" if @receipt["upload_id"] && @receipt["upload_id"] != upload["id"]
    end
    upload
  end

  def asset_file(upload)
    response = get("/v1/buildUploads/#{upload.fetch('id')}", "include" => "assetFile,build")
    id = response.dig("data", "relationships", "assetFile", "data", "id")
    file = response.fetch("included", []).find { |r| r["type"] == "buildUploadFiles" && r["id"] == id }
    raise "Upload file ID changed" if file && @receipt["upload_file_id"] && file["id"] != @receipt["upload_file_id"]
    file
  end

  def digest_matches?(file)
    file && file.dig("attributes", "sourceFileChecksums", "file") == {"algorithm" => "SHA_256", "hash" => @manifest.fetch("ipa_sha256")}
  end

  def record_intent!
    raise "Build already exists without a verifiable upload receipt" if find_build
    raise "Upload already exists; use the recorded upload adapter to resume" if exact_upload
    checkpoint("status" => "upload-intent", "adapter" => @config.fetch("upload_adapter"))
  end

  def reconcile_accepted!
    upload = exact_upload
    return false unless upload
    file = asset_file(upload)
    raise "Cannot authenticate existing upload bytes; refusing duplicate transfer" unless digest_matches?(file) || (@receipt["status"] == "transferred" && @receipt["adapter"] == "transporter")
    raise "Missing Apple upload file identity" unless file && file["id"]
    checkpoint("status" => "transferred", "upload_id" => upload.fetch("id"), "upload_file_id" => file.fetch("id"))
    true
  end

  def transfer_part(ipa, operation)
    uri = URI(operation.fetch("url"))
    # Upload URLs are Apple-issued bearer capabilities; never log them or forward JWTs.
    raise "Unsafe upload operation" unless uri.scheme == "https" && uri.port == 443 && operation.fetch("method") == "PUT" && !uri.userinfo
    offset, length = Integer(operation.fetch("offset")), Integer(operation.fetch("length"))
    raise "Invalid upload range" unless offset >= 0 && length > 0 && offset + length <= File.size(ipa)
    request = Net::HTTP::Put.new(uri)
    operation.fetch("requestHeaders", []).each do |header|
      raise "Unexpected credential forwarding header" if %w[authorization cookie host].include?(header.fetch("name").downcase)
      request[header.fetch("name")] = header.fetch("value")
    end
    # Bounded range buffer; large upload operations are streamed from a limited IO.
    File.open(ipa, "rb") do |input|
      input.seek(offset)
      request.content_length = length
      request.body_stream = RangeIO.new(input, length)
      response = Net::HTTP.start(uri.host, uri.port, use_ssl: true, open_timeout: 20, read_timeout: 300, write_timeout: 300) { |http| http.request(request) }
      raise "Apple upload part failed: HTTP #{response.code}" unless response.is_a?(Net::HTTPSuccess)
    end
  end

  class RangeIO
    def initialize(io, remaining); @io, @remaining = io, remaining; end
    def read(length = nil, out = nil)
      return nil if @remaining.zero?
      value = @io.read([length || @remaining, @remaining].min)
      raise "Unexpected end of IPA" unless value
      @remaining -= value.bytesize
      out ? out.replace(value) : value
    end
  end

  def upload!(ipa)
    raise "IPA checksum mismatch" unless Digest::SHA256.file(ipa).hexdigest == @manifest.fetch("ipa_sha256")
    upload = exact_upload
    unless upload
      raise "Build exists without matching upload" if find_build
      checkpoint("status" => "upload-intent", "adapter" => "build-uploads")
      upload = mutate("POST", "/v1/buildUploads", {"data" => {"type" => "buildUploads", "attributes" => {
        "cfBundleShortVersionString" => @manifest.fetch("version"), "cfBundleVersion" => @manifest.fetch("build_number"), "platform" => "IOS"},
        "relationships" => {"app" => {"data" => {"type" => "apps", "id" => app_id}}}}}).fetch("data")
    end
    checkpoint("upload_id" => upload.fetch("id"))
    file = asset_file(upload)
    unless file
      file = mutate("POST", "/v1/buildUploadFiles", {"data" => {"type" => "buildUploadFiles", "attributes" => {
        "assetType" => "ASSET", "fileName" => "application.ipa", "fileSize" => File.size(ipa), "uti" => "com.apple.ipa"},
        "relationships" => {"buildUpload" => {"data" => {"type" => "buildUploads", "id" => upload.fetch("id")}}}}}).fetch("data")
    end
    checkpoint("upload_file_id" => file.fetch("id"))
    attrs = file.fetch("attributes")
    raise "Upload file size changed" unless attrs.fetch("fileSize") == File.size(ipa)
    state = attrs.dig("assetDeliveryState", "state")
    raise "Apple rejected upload file" if state == "FAILED"
    if %w[UPLOAD_COMPLETE COMPLETE].include?(state)
      raise "Existing upload checksum differs" unless digest_matches?(file)
      checkpoint("status" => "transferred")
      return
    end
    raise "Unexpected upload file state" unless state == "AWAITING_UPLOAD"
    # Repeating PUT for an uncommitted reservation is safe, even after a lost response.
    ranges = attrs.fetch("uploadOperations").sort_by { |op| Integer(op.fetch("offset")) }
    end_offset = 0
    ranges.each do |operation|
      raise "Noncontiguous upload operations" unless Integer(operation.fetch("offset")) == end_offset
      transfer_part(ipa, operation)
      end_offset += Integer(operation.fetch("length"))
    end
    raise "Incomplete upload operations" unless end_offset == File.size(ipa)
    mutate("PATCH", "/v1/buildUploadFiles/#{file.fetch('id')}", {"data" => {"type" => "buildUploadFiles", "id" => file.fetch("id"),
      "attributes" => {"uploaded" => true, "sourceFileChecksums" => {"file" => {"algorithm" => "SHA_256", "hash" => @manifest.fetch("ipa_sha256")}}}}})
    checkpoint("status" => "transferred")
  end

  def wait_for_verified_processing!(timeout: 3600, interval: 30)
    deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + timeout
    loop do
      upload = exact_upload
      if upload && upload_state(upload) == "COMPLETE"
        file = asset_file(upload)
        # Transporter can report MD5 only. Its successful transfer receipt is required;
        # an unrelated existing build is never adopted merely by matching version.
        transferred_here = @receipt["status"] == "transferred" && @receipt["adapter"] == "transporter"
        raise "Upload digest cannot be authenticated" unless digest_matches?(file) || transferred_here
        raise "Missing Apple upload file identity" unless file && file["id"]
        build_id = get("/v1/buildUploads/#{upload.fetch('id')}").dig("data", "relationships", "build", "data", "id")
        build = find_build
        if build && build["id"] == build_id && build.dig("attributes", "processingState") == "VALID"
          checkpoint("upload_id" => upload.fetch("id"), "upload_file_id" => file.fetch("id"), "app_store_build_id" => build_id, "status" => "processed")
          return @manifest.slice("source_sha", "version", "build_number", "ipa_sha256").merge(
            "upload_id" => upload.fetch("id"), "upload_file_id" => file.fetch("id"), "app_store_build_id" => build_id, "processing_state" => "VALID")
        end
        raise "Processed build conflicts with upload" if build && build_id && build["id"] != build_id
      end
      raise "Timed out waiting for verified Apple upload/build" if Process.clock_gettime(Process::CLOCK_MONOTONIC) >= deadline
      sleep(interval)
    end
  end
end
