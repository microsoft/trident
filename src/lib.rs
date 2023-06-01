pub mod config;

use protobufs::*;
use tonic::{Request, Response, Status};

pub use protobufs::greeter_server::GreeterServer;

mod protobufs {
    tonic::include_proto!("trident");
}

#[derive(Default)]
pub struct GreeterImpl {}

#[tonic::async_trait]
impl greeter_server::Greeter for GreeterImpl {
    async fn say_hello(
        &self,
        request: Request<HelloRequest>,
    ) -> Result<Response<HelloReply>, Status> {
        println!("Got a request from {:?}", request.remote_addr());

        let reply = HelloReply {
            message: format!("Hello {}!", request.into_inner().name),
        };
        Ok(Response::new(reply))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
