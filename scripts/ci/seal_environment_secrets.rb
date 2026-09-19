# One-time owner-triggered migration. Only GitHub-encrypted ciphertext leaves the runner.
require 'base64'
require 'json'
require 'jwt'
require 'net/http'
require 'openssl'
require 'fiddle/import'
require 'fileutils'
module Sodium
  extend Fiddle::Importer
  dlload 'libsodium.so.23'
  extern 'int sodium_init()'
  extern 'int crypto_box_seal(void*, const void*, unsigned long long, const void*)'
end
raise 'libsodium initialization failed' if Sodium.sodium_init < 0
keys=JSON.parse(File.read(ENV.fetch('ENVIRONMENT_PUBLIC_KEYS')))
map={
  'signing'=>%w[MATCH_PASSWORD MATCH_SSH_PRIVATE_KEY],
  'testflight'=>%w[APP_STORE_CONNECT_API_KEY_ID APP_STORE_CONNECT_API_KEY_ISSUER_ID APP_STORE_CONNECT_API_KEY_CONTENT],
  'app-store-staging'=>%w[APP_STORE_CONNECT_API_KEY_ID APP_STORE_CONNECT_API_KEY_ISSUER_ID APP_STORE_CONNECT_API_KEY_CONTENT],
  'production'=>%w[APP_STORE_CONNECT_API_KEY_ID APP_STORE_CONNECT_API_KEY_ISSUER_ID APP_STORE_CONNECT_API_KEY_CONTENT],
  'app-store-observe'=>%w[APP_STORE_CONNECT_API_KEY_ID APP_STORE_CONNECT_API_KEY_ISSUER_ID APP_STORE_CONNECT_API_KEY_CONTENT],
  'release-publishing'=>%w[RELEASE_APP_ID RELEASE_APP_PRIVATE_KEY],
  'screenshot-publishing'=>%w[RELEASE_APP_ID RELEASE_APP_PRIVATE_KEY]
}
sealed=[]
map.each do |environment,names|
  info=keys.fetch(environment)
  public_key=Base64.strict_decode64(info.fetch('key'))
  raise 'Unexpected GitHub public key length' unless public_key.bytesize==32
  names.each do |name|
    value=ENV.fetch(name)
    raise "Missing required secret #{name}" if value.empty?
    ciphertext="\0"*(value.bytesize+48)
    raise 'GitHub secret encryption failed' unless Sodium.crypto_box_seal(ciphertext,value,value.bytesize,public_key)==0
    sealed << {environment:environment,name:name,key_id:info.fetch('key_id'),encrypted_value:Base64.strict_encode64(ciphertext)}
  end
end
private_key=OpenSSL::PKey.read(ENV.fetch('RELEASE_APP_PRIVATE_KEY').gsub('\\n',"\n"))
token=JWT.encode({iat:Time.now.to_i-30,exp:Time.now.to_i+300,iss:ENV.fetch('RELEASE_APP_ID')},private_key,'RS256')
uri=URI('https://api.github.com/app')
request=Net::HTTP::Get.new(uri);request['Authorization']="Bearer #{token}";request['Accept']='application/vnd.github+json';request['User-Agent']='ios-release-bootstrap'
response=Net::HTTP.start(uri.host,uri.port,use_ssl:true,open_timeout:15,read_timeout:30){|http|http.request(request)}
raise "Could not inspect release App: HTTP #{response.code}" unless response.is_a?(Net::HTTPSuccess)
app=JSON.parse(response.body)
FileUtils.mkdir_p('build/bootstrap')
File.write('build/bootstrap/sealed-secrets.json',JSON.pretty_generate(sealed))
File.write('build/bootstrap/release-app.json',JSON.pretty_generate(app.slice('id','slug','permissions')))
puts "Encrypted #{sealed.length} environment secrets; no plaintext exported."
