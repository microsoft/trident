# Trident Container Installer ISO Test Image

This image is used in the Trident test pipelines. At startup, it loads a specified version 
of a container, which must be provided. Trident runs from this container. The configuration 
for Trident can be patched into the ISO by replacing it with the placeholder file 
(config-placeholder).
