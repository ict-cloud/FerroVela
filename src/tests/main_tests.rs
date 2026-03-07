use crate::config::{load_config, Config, ExceptionsConfig, ProxyConfig, UpstreamConfig};
use crate::pac::PacEngine;
use crate::proxy::Proxy;
use std::fs;
use std::io::Write;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[test]
fn test_load_config() {
    let config_content = r#"
[proxy]
port = 8080
pac_file = "http://wpad/wpad.dat"

[upstream]
auth_type = "basic"
username = "user"
password = "password"
proxy_url = "10.0.0.1:3128"

[exceptions]
hosts = ["localhost", "127.0.0.1", "*.internal"]
"#;
    let file_path = "test_config.toml";
    let mut file = fs::File::create(file_path).expect("Failed to create test config file");
    file.write_all(config_content.as_bytes())
        .expect("Failed to write to test config file");

    let config = load_config(file_path).expect("Failed to load config");

    assert_eq!(config.proxy.port, 8080);
    assert_eq!(
        config.proxy.pac_file,
        Some("http://wpad/wpad.dat".to_string())
    );

    let upstream = config.upstream.unwrap();
    assert_eq!(upstream.auth_type, "basic");
    assert_eq!(upstream.username, Some("user".to_string()));
    assert_eq!(upstream.password, Some("password".to_string()));
    assert_eq!(upstream.proxy_url, Some("10.0.0.1:3128".to_string()));

    let exceptions = config.exceptions.unwrap();
    assert_eq!(exceptions.hosts.len(), 3);
    assert_eq!(exceptions.hosts[0], "localhost");

    // Cleanup
    fs::remove_file(file_path).expect("Failed to delete test config file");
}

#[test]
fn test_default_port_config() {
    let config_content = r#"
[proxy]
# port is omitted, should default to 3128
pac_file = "http://wpad/wpad.dat"

[upstream]
auth_type = "basic"
username = "user"
password = "password"
proxy_url = "10.0.0.1:3128"
"#;
    let file_path = "test_config_default_port.toml";
    let mut file = fs::File::create(file_path).expect("Failed to create test config file");
    file.write_all(config_content.as_bytes())
        .expect("Failed to write to test config file");

    let config = load_config(file_path).expect("Failed to load config");

    assert_eq!(config.proxy.port, 3128);

    // Cleanup
    fs::remove_file(file_path).expect("Failed to delete test config file");
}

#[tokio::test]
async fn test_pac_engine() {
    let pac_content = r#"
        function FindProxyForURL(url, host) {
            if (host == "localhost") return "DIRECT";
            if (shExpMatch(host, "*.internal")) return "PROXY 10.0.0.1:8080";
            return "PROXY 192.168.1.1:3128";
        }
    "#;
    let pac_path = "test.pac";
    let mut file = fs::File::create(pac_path).expect("Failed to create PAC file");
    file.write_all(pac_content.as_bytes())
        .expect("Failed to write PAC file");

    let engine = PacEngine::new(pac_path)
        .await
        .expect("Failed to create PacEngine");

    // Test localhost -> DIRECT
    let proxy = engine
        .find_proxy("http://localhost/foo", "localhost")
        .await
        .expect("PAC failed");
    assert_eq!(proxy, "DIRECT");

    // Test internal -> PROXY 10.0.0.1:8080
    let proxy = engine
        .find_proxy("http://foo.internal/bar", "foo.internal")
        .await
        .expect("PAC failed");
    assert_eq!(proxy, "PROXY 10.0.0.1:8080");

    // Test other -> PROXY 192.168.1.1:3128
    let proxy = engine
        .find_proxy("http://google.com", "google.com")
        .await
        .expect("PAC failed");
    assert_eq!(proxy, "PROXY 192.168.1.1:3128");

    fs::remove_file(pac_path).expect("Failed to remove PAC file");
}

#[tokio::test]
async fn test_pac_is_plain_host_name() {
    let pac_content = r#"
        function FindProxyForURL(url, host) {
            if (isPlainHostName(host)) return "DIRECT";
            return "PROXY 10.0.0.1:8080";
        }
    "#;
    let pac_path = "test_plain_host.pac";
    let mut file = fs::File::create(pac_path).expect("Failed to create PAC file");
    file.write_all(pac_content.as_bytes())
        .expect("Failed to write PAC file");

    let engine = PacEngine::new(pac_path)
        .await
        .expect("Failed to create PacEngine");

    // Plain hostname (no dots) -> DIRECT
    let proxy = engine
        .find_proxy("http://intranet/page", "intranet")
        .await
        .expect("PAC failed");
    assert_eq!(proxy, "DIRECT");

    // FQDN (has dots) -> PROXY
    let proxy = engine
        .find_proxy("http://www.example.com/page", "www.example.com")
        .await
        .expect("PAC failed");
    assert_eq!(proxy, "PROXY 10.0.0.1:8080");

    fs::remove_file(pac_path).expect("Failed to remove PAC file");
}

#[tokio::test]
async fn test_pac_dns_domain_is() {
    let pac_content = r#"
        function FindProxyForURL(url, host) {
            if (dnsDomainIs(host, ".example.com")) return "DIRECT";
            return "PROXY 10.0.0.1:8080";
        }
    "#;
    let pac_path = "test_dns_domain.pac";
    let mut file = fs::File::create(pac_path).expect("Failed to create PAC file");
    file.write_all(pac_content.as_bytes())
        .expect("Failed to write PAC file");

    let engine = PacEngine::new(pac_path)
        .await
        .expect("Failed to create PacEngine");

    // Host in .example.com -> DIRECT
    let proxy = engine
        .find_proxy("http://www.example.com/", "www.example.com")
        .await
        .expect("PAC failed");
    assert_eq!(proxy, "DIRECT");

    // Host not in .example.com -> PROXY
    let proxy = engine
        .find_proxy("http://www.other.org/", "www.other.org")
        .await
        .expect("PAC failed");
    assert_eq!(proxy, "PROXY 10.0.0.1:8080");

    fs::remove_file(pac_path).expect("Failed to remove PAC file");
}

#[tokio::test]
async fn test_pac_local_host_or_domain_is() {
    let pac_content = r#"
        function FindProxyForURL(url, host) {
            if (localHostOrDomainIs(host, "www.example.com")) return "DIRECT";
            return "PROXY 10.0.0.1:8080";
        }
    "#;
    let pac_path = "test_local_host_domain.pac";
    let mut file = fs::File::create(pac_path).expect("Failed to create PAC file");
    file.write_all(pac_content.as_bytes())
        .expect("Failed to write PAC file");

    let engine = PacEngine::new(pac_path)
        .await
        .expect("Failed to create PacEngine");

    // Exact match -> DIRECT
    let proxy = engine
        .find_proxy("http://www.example.com/", "www.example.com")
        .await
        .expect("PAC failed");
    assert_eq!(proxy, "DIRECT");

    // Unqualified hostname matches -> DIRECT
    let proxy = engine
        .find_proxy("http://www/", "www")
        .await
        .expect("PAC failed");
    assert_eq!(proxy, "DIRECT");

    // Different host -> PROXY
    let proxy = engine
        .find_proxy("http://other.example.com/", "other.example.com")
        .await
        .expect("PAC failed");
    assert_eq!(proxy, "PROXY 10.0.0.1:8080");

    fs::remove_file(pac_path).expect("Failed to remove PAC file");
}

#[tokio::test]
async fn test_pac_is_in_net() {
    let pac_content = r#"
        function FindProxyForURL(url, host) {
            if (isInNet(host, "10.0.0.0", "255.0.0.0")) return "DIRECT";
            if (isInNet(host, "192.168.1.0", "255.255.255.0")) return "PROXY internal:8080";
            return "PROXY 10.0.0.1:8080";
        }
    "#;
    let pac_path = "test_is_in_net.pac";
    let mut file = fs::File::create(pac_path).expect("Failed to create PAC file");
    file.write_all(pac_content.as_bytes())
        .expect("Failed to write PAC file");

    let engine = PacEngine::new(pac_path)
        .await
        .expect("Failed to create PacEngine");

    // IP in 10.0.0.0/8 -> DIRECT
    let proxy = engine
        .find_proxy("http://10.1.2.3/", "10.1.2.3")
        .await
        .expect("PAC failed");
    assert_eq!(proxy, "DIRECT");

    // IP in 192.168.1.0/24 -> internal proxy
    let proxy = engine
        .find_proxy("http://192.168.1.50/", "192.168.1.50")
        .await
        .expect("PAC failed");
    assert_eq!(proxy, "PROXY internal:8080");

    // IP outside both ranges -> external proxy
    let proxy = engine
        .find_proxy("http://8.8.8.8/", "8.8.8.8")
        .await
        .expect("PAC failed");
    assert_eq!(proxy, "PROXY 10.0.0.1:8080");

    fs::remove_file(pac_path).expect("Failed to remove PAC file");
}

#[tokio::test]
async fn test_pac_dns_domain_levels() {
    let pac_content = r#"
        function FindProxyForURL(url, host) {
            var levels = dnsDomainLevels(host);
            if (levels == 0) return "DIRECT";
            if (levels == 1) return "PROXY local:8080";
            return "PROXY remote:8080";
        }
    "#;
    let pac_path = "test_dns_levels.pac";
    let mut file = fs::File::create(pac_path).expect("Failed to create PAC file");
    file.write_all(pac_content.as_bytes())
        .expect("Failed to write PAC file");

    let engine = PacEngine::new(pac_path)
        .await
        .expect("Failed to create PacEngine");

    // No dots -> DIRECT
    let proxy = engine
        .find_proxy("http://intranet/", "intranet")
        .await
        .expect("PAC failed");
    assert_eq!(proxy, "DIRECT");

    // One dot -> local proxy
    let proxy = engine
        .find_proxy("http://example.com/", "example.com")
        .await
        .expect("PAC failed");
    assert_eq!(proxy, "PROXY local:8080");

    // Two dots -> remote proxy
    let proxy = engine
        .find_proxy("http://www.example.com/", "www.example.com")
        .await
        .expect("PAC failed");
    assert_eq!(proxy, "PROXY remote:8080");

    fs::remove_file(pac_path).expect("Failed to remove PAC file");
}

#[tokio::test]
async fn test_pac_convert_addr() {
    let pac_content = r#"
        function FindProxyForURL(url, host) {
            var addr = convert_addr(host);
            if (addr == 3232235876) return "DIRECT";
            return "PROXY 10.0.0.1:8080";
        }
    "#;
    // 192.168.0.100 = (192 << 24) + (168 << 16) + (0 << 8) + 100 = 3232235620
    // 192.168.1.100 = (192 << 24) + (168 << 16) + (1 << 8) + 100 = 3232235876
    let pac_path = "test_convert_addr.pac";
    let mut file = fs::File::create(pac_path).expect("Failed to create PAC file");
    file.write_all(pac_content.as_bytes())
        .expect("Failed to write PAC file");

    let engine = PacEngine::new(pac_path)
        .await
        .expect("Failed to create PacEngine");

    // 192.168.1.100 = 3232235876 -> DIRECT
    let proxy = engine
        .find_proxy("http://192.168.1.100/", "192.168.1.100")
        .await
        .expect("PAC failed");
    assert_eq!(proxy, "DIRECT");

    // Other IP -> PROXY
    let proxy = engine
        .find_proxy("http://10.0.0.1/", "10.0.0.1")
        .await
        .expect("PAC failed");
    assert_eq!(proxy, "PROXY 10.0.0.1:8080");

    fs::remove_file(pac_path).expect("Failed to remove PAC file");
}

#[tokio::test]
async fn test_pac_weekday_date_time_range_stubs() {
    let pac_content = r#"
        function FindProxyForURL(url, host) {
            if (weekdayRange("MON", "FRI")) {
                if (dateRange(1, "JAN", 2020, 31, "DEC", 2030)) {
                    if (timeRange(8, 20)) {
                        return "PROXY work:8080";
                    }
                }
            }
            return "DIRECT";
        }
    "#;
    let pac_path = "test_time_stubs.pac";
    let mut file = fs::File::create(pac_path).expect("Failed to create PAC file");
    file.write_all(pac_content.as_bytes())
        .expect("Failed to write PAC file");

    let engine = PacEngine::new(pac_path)
        .await
        .expect("Failed to create PacEngine");

    // All time stubs return true, so we should get the work proxy
    let proxy = engine
        .find_proxy("http://example.com/", "example.com")
        .await
        .expect("PAC failed");
    assert_eq!(proxy, "PROXY work:8080");

    fs::remove_file(pac_path).expect("Failed to remove PAC file");
}

#[tokio::test]
async fn test_pac_combined_helpers() {
    // A realistic PAC script using multiple helper functions together
    let pac_content = r#"
        function FindProxyForURL(url, host) {
            if (isPlainHostName(host)) return "DIRECT";
            if (dnsDomainIs(host, ".internal.corp")) return "DIRECT";
            if (isInNet(host, "172.16.0.0", "255.240.0.0")) return "DIRECT";
            if (dnsDomainLevels(host) > 2) return "PROXY deep:8080";
            if (shExpMatch(host, "*.example.com")) return "PROXY example:8080";
            return "PROXY default:8080";
        }
    "#;
    let pac_path = "test_combined.pac";
    let mut file = fs::File::create(pac_path).expect("Failed to create PAC file");
    file.write_all(pac_content.as_bytes())
        .expect("Failed to write PAC file");

    let engine = PacEngine::new(pac_path)
        .await
        .expect("Failed to create PacEngine");

    // Plain hostname -> DIRECT
    let proxy = engine
        .find_proxy("http://myserver/", "myserver")
        .await
        .expect("PAC failed");
    assert_eq!(proxy, "DIRECT");

    // Internal corp domain -> DIRECT
    let proxy = engine
        .find_proxy("http://app.internal.corp/", "app.internal.corp")
        .await
        .expect("PAC failed");
    assert_eq!(proxy, "DIRECT");

    // IP in 172.16.0.0/12 -> DIRECT
    let proxy = engine
        .find_proxy("http://172.20.1.5/", "172.20.1.5")
        .await
        .expect("PAC failed");
    assert_eq!(proxy, "DIRECT");

    // Deep domain (3+ levels) -> deep proxy
    let proxy = engine
        .find_proxy("http://a.b.c.d.com/", "a.b.c.d.com")
        .await
        .expect("PAC failed");
    assert_eq!(proxy, "PROXY deep:8080");

    // *.example.com -> example proxy
    let proxy = engine
        .find_proxy("http://www.example.com/", "www.example.com")
        .await
        .expect("PAC failed");
    assert_eq!(proxy, "PROXY example:8080");

    // Anything else -> default proxy
    let proxy = engine
        .find_proxy("http://google.com/", "google.com")
        .await
        .expect("PAC failed");
    assert_eq!(proxy, "PROXY default:8080");

    fs::remove_file(pac_path).expect("Failed to remove PAC file");
}

// Helpers
async fn start_target_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind target");
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        loop {
            if let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0; 1024];
                    loop {
                        match socket.read(&mut buf).await {
                            Ok(n) if n == 0 => return,
                            Ok(n) => {
                                if socket.write_all(&buf[..n]).await.is_err() {
                                    return;
                                }
                            }
                            Err(_) => return,
                        }
                    }
                });
            }
        }
    });
    port
}

async fn start_proxy(
    upstream: Option<UpstreamConfig>,
    exceptions: Option<ExceptionsConfig>,
) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind proxy");
    let port = listener.local_addr().unwrap().port();

    let config = Config {
        proxy: ProxyConfig {
            port,
            pac_file: None,
            allow_private_ips: true,
        },
        upstream,
        exceptions,
    };

    let proxy = Proxy::new(Arc::new(config), None, None);
    tokio::spawn(async move {
        let _ = proxy.run_with_listener(listener).await;
    });

    port
}

#[tokio::test]
async fn test_proxy_direct() {
    let target_port = start_target_server().await;
    let proxy_port = start_proxy(None, None).await;

    let mut client = TcpStream::connect(format!("127.0.0.1:{}", proxy_port))
        .await
        .expect("Failed to connect to proxy");

    let connect_req = format!(
        "CONNECT 127.0.0.1:{} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
        target_port, target_port
    );
    client.write_all(connect_req.as_bytes()).await.unwrap();

    let mut buf = [0; 1024];
    let n = client.read(&mut buf).await.unwrap();
    let resp = String::from_utf8_lossy(&buf[..n]);
    assert!(resp.contains("200"), "Expected 200 OK, got {}", resp);

    client.write_all(b"Hello").await.unwrap();
    let n = client.read(&mut buf).await.unwrap();
    assert_eq!(&buf[..n], b"Hello");
}

#[tokio::test]
async fn test_proxy_upstream() {
    let target_port = start_target_server().await;

    // Mock Upstream Proxy
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_port = upstream_listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        loop {
            if let Ok((mut socket, _)) = upstream_listener.accept().await {
                let target_port = target_port; // Capture
                tokio::spawn(async move {
                    let mut buf = [0; 1024];
                    let n = socket.read(&mut buf).await.unwrap();
                    let req = String::from_utf8_lossy(&buf[..n]);

                    if req.starts_with("CONNECT") {
                        socket.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await.unwrap();

                        let mut target = TcpStream::connect(format!("127.0.0.1:{}", target_port))
                            .await
                            .unwrap();
                        let _ = tokio::io::copy_bidirectional(&mut socket, &mut target).await;
                    }
                });
            }
        }
    });

    let upstream_config = UpstreamConfig {
        auth_type: "none".to_string(),
        username: None,
        password: None,
        domain: None,
        workstation: None,
        proxy_url: Some(format!("127.0.0.1:{}", upstream_port)),
    };

    let proxy_port = start_proxy(Some(upstream_config), None).await;

    let mut client = TcpStream::connect(format!("127.0.0.1:{}", proxy_port))
        .await
        .unwrap();
    let connect_req = format!(
        "CONNECT 127.0.0.1:{} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
        target_port, target_port
    );
    client.write_all(connect_req.as_bytes()).await.unwrap();

    let mut buf = [0; 1024];
    let n = client.read(&mut buf).await.unwrap();
    let resp = String::from_utf8_lossy(&buf[..n]);
    assert!(
        resp.contains("200"),
        "Expected 200 OK from upstream via proxy, got {}",
        resp
    );

    client.write_all(b"UpstreamTest").await.unwrap();
    let n = client.read(&mut buf).await.unwrap();
    assert_eq!(&buf[..n], b"UpstreamTest");
}

#[tokio::test]
async fn test_proxy_exceptions() {
    let target_port = start_target_server().await;

    // Upstream that fails (binds but sends 500)
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_port = upstream_listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = upstream_listener.accept().await {
            let _ = socket
                .write_all(b"HTTP/1.1 500 Internal Server Error\r\n\r\n")
                .await;
        }
    });

    let upstream_config = UpstreamConfig {
        auth_type: "none".to_string(),
        username: None,
        password: None,
        domain: None,
        workstation: None,
        proxy_url: Some(format!("127.0.0.1:{}", upstream_port)),
    };

    let exceptions = ExceptionsConfig {
        hosts: vec!["127.0.0.1".to_string()],
    };

    let proxy_port = start_proxy(Some(upstream_config), Some(exceptions)).await;

    let mut client = TcpStream::connect(format!("127.0.0.1:{}", proxy_port))
        .await
        .unwrap();
    // Use 127.0.0.1 to match exception
    let connect_req = format!(
        "CONNECT 127.0.0.1:{} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
        target_port, target_port
    );
    client.write_all(connect_req.as_bytes()).await.unwrap();

    let mut buf = [0; 1024];
    let n = client.read(&mut buf).await.unwrap();
    let resp = String::from_utf8_lossy(&buf[..n]);
    assert!(resp.contains("200"), "Expected 200 OK, got {}", resp);

    // If it went upstream, the next read/write would fail because the tunnel is broken (upstream sent 500 then closed).
    // If it went direct, it connects to Echo Server.

    client.write_all(b"ExceptionTest").await.unwrap();
    let n = client.read(&mut buf).await.unwrap();
    assert_eq!(
        &buf[..n],
        b"ExceptionTest",
        "Traffic should flow direct to target"
    );
}
