# websocket_mq
基于websocket的redis消息转发网关

---
#### 介绍
b/s架构中，当服务端需要一个能主动向web客户端发送消息的渠道时，比较好的选择是websocket技术。在基于spring boot框架开发下进行开发，spring boot也提供了websocket组件，然而，spring boot的网络连接是传统基于多线程阻塞IO模式，此种方式在有大量客户端连接但读写并不是很频繁的场景下并不适合。

websocket_mq是基于rust/tokio技术框架下开发的websocket服务端中间件，特别适合于大量客户端连接、读写并不是很频繁的场景，轻松实现C10K连接。

#### 项目地址
<https://github.com/kivensoft/websocket_mq>

###### 技术框架
* rust 开发语言
* tokio 异步运行时
* tokio-tungstenite 基于tokio的websocket库
* redis 目前最好的redis客户端库

###### 源代码下载
`git clone git@github.com:kivensoft/websocket_mq.git`
###### 编译
`cargo build`
###### 显示帮助信息
`websocket_mq -h`
