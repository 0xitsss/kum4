use reqwest::Client;

pub fn new(tor_proxy: &str) -> crate::error::Result<Client> {
    let proxy = reqwest::Proxy::all(tor_proxy)?;
    Ok(Client::builder().proxy(proxy).build()?)
}
