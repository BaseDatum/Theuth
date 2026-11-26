"""cloud-nodes: hosted theuth nodes (design doc 224). The tools; the logic is cloud_nodes.py.

A CloudError (or any exception) in a tool reaches the model as `isError: true` with its message.
"""

import cloud_nodes as cn
from theuth_plugin import plugin


@plugin.tool("providers")
def providers(_):
    return cn.providers()


@plugin.tool("create")
def create(req):
    return cn.create(req.args or {})


@plugin.tool("status")
def status(req):
    return cn.status(req.args or {})


@plugin.tool("list")
def list_nodes(req):
    return cn.list_nodes(req.args or {})


@plugin.tool("destroy")
def destroy(req):
    return cn.destroy(req.args or {})


if __name__ == "__main__":
    plugin.run()
