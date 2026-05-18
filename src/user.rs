use axum::{
    body::Body,
    http::{Request, Response, StatusCode},
};
use futures::future::BoxFuture;
use std::path::{Path, PathBuf};
use tower_http::auth::AsyncAuthorizeRequest;

#[derive(Clone)]
pub(crate) struct Authenticator {
    data_root: PathBuf,
}

impl Authenticator {
    pub(crate) fn new(data_root: PathBuf) -> Self {
        Self { data_root }
    }
}

impl<B> AsyncAuthorizeRequest<B> for Authenticator
where
    B: Send + 'static,
{
    type RequestBody = B;
    type ResponseBody = Body;
    type Future = BoxFuture<'static, Result<Request<B>, Response<Self::ResponseBody>>>;

    fn authorize(&mut self, request: Request<B>) -> Self::Future {
        let data_root = self.data_root.clone();
        Box::pin(async move {
            if let Some((user, mut request)) = check_auth(&data_root, request).await {
                request.extensions_mut().insert(user);

                Ok(request)
            } else {
                let unauthorized_response = Response::builder()
                    .status(StatusCode::UNAUTHORIZED)
                    .body(Body::empty())
                    .unwrap();

                Err(unauthorized_response)
            }
        })
    }
}

async fn check_auth<B>(data_root: &Path, request: Request<B>) -> Option<(User, Request<B>)> {
    Some((
        User {
            data_root: data_root.join("myuser"),
        },
        request,
    ))
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct User {
    data_root: PathBuf,
}

impl User {
    pub(crate) fn data_root(&self) -> &Path {
        &self.data_root
    }
}
