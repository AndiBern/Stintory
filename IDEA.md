# Rundenzeit Vision
As a passionate sim racer I want a tool to track my lap times to be able to review them afterwards to understand where I can improve. 
Currently, there are only apps that send your data to their servers to be able to compare it to all other users.
But I want a tool that does not send my data to any company. I want tool that saves all data locally or in a self-hosted server.

# Architecture
There should be two components: Client and Server

## Client
- The app that runs on the machine where the metrics should be collected
- Saves all the data locally with the option to sync it to a server
- Displays all tracked sessions and option to compare two laps
- Runs on Windows and Linux
- Option to export a selection of sessions/laps
- Option to import sessions/laps (also from other users) to compare them against own laps

## Server
- A self-hostable server that multiple user can sync their data to
- Runs as docker container
- User can select if they want to share their data by default with other users
- User can choose sharing on a per sessions/laps basis

# Game Support
- iRacing (runs only Windows as of now)
- F1 25/26 (runs only Windows as of now)
- Le Mans Ultimate

# Competitors
These are some apps that do exactly what I want, but with their servers.
- TrackTitan https://www.tracktitan.io/
- Trophi.ai https://www.trophi.ai/sim-racing/
- Garage61 https://garage61.net/