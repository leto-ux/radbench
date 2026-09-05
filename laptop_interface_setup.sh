#!/bin/sh

sudo ip addr flush dev enp56s0f4u1
sudo ip addr add 192.168.43.100/24 dev enp56s0f4u1
sudo ip link set enp56s0f4u1 up
firewall-cmd --get-active-zones 2>/dev/null | grep -B 1 enp56s0f4u1
sudo firewall-cmd --zone=trusted --change-interface=enp56s0f4u1
