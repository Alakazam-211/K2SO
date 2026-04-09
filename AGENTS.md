<!-- K2SO:BEGIN -->
# K2SO Skill

This workspace (K2SO) is managed by K2SO. You can use these commands to interact with the agent system.

## Send Work to a Workspace

Send a task to a workspace's manager for triage and execution:
```
k2so msg <workspace-name>:inbox "description of work needed"
k2so msg --wake <workspace-name>:inbox "urgent — wake the agent"
```

## View Activity Feed

See recent agent activity in this workspace:
```
k2so feed
```

## View Connections

See which workspaces are connected:
```
k2so connections list
```

## Create a Work Item

Add work to this workspace's inbox for the manager to triage:
```
k2so work create --title "Fix login bug" --body "Users can't log in after password reset" --source issue
```

<!-- K2SO:END -->